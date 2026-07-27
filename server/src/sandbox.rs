//! OS-level sandbox layer (`--sandbox-manifest`), powered by the [nono] crate.
//!
//! The flag takes a nono capability manifest — the JSON format defined by
//! nono's `capability-manifest` schema — and passes it to nono verbatim:
//! filesystem grants, network mode, and port allowlists all use nono's own
//! semantics via its `CapabilityManifest -> CapabilitySet` conversion. This
//! server adds no derivation or defaults on top; the manifest must grant
//! everything the process needs, including its own storage directories and
//! the system library paths.
//!
//! The sandbox is applied before the tokio runtime or the V8 platform spawn
//! a single thread (Landlock confines the calling thread and everything it
//! spawns afterwards), and is irreversible for the lifetime of the process.
//! It sits *underneath* the OPA policy layer: policies decide what JS may ask
//! for; the OS sandbox bounds what the process can do even if V8, the policy
//! chain, or the server itself is compromised.
//!
//! Fail-closed contract, in two parts:
//! - If the platform cannot enforce the manifest (unsupported OS, Landlock
//!   disabled), startup aborts rather than running unconfined.
//! - Manifest features that only work under nono's CLI supervisor — network
//!   `proxy` mode (and its `allow_domains`/`endpoints` filtering), `dns:
//!   false`, `credentials`, `rollback`, filesystem `deny` rules, `exec_strategy:
//!   supervised` — are rejected at startup rather than silently ignored:
//!   this server applies the sandbox in-process, where only kernel-expressible
//!   rules (path grants, port allowlists, block-all) exist.
//!
//! [nono]: https://github.com/nolabs-ai/nono

use anyhow::{Context, Result};

/// Validate that the manifest stays within the subset nono can enforce
/// in-process (kernel rules only — no supervising parent, no proxy).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn reject_supervisor_features(manifest: &nono::manifest::CapabilityManifest) -> Result<()> {
    use nono::manifest::{ExecStrategy, NetworkMode};

    let mut unsupported: Vec<&str> = Vec::new();

    if let Some(fs) = &manifest.filesystem {
        if !fs.deny.is_empty() {
            unsupported.push("filesystem.deny (Landlock is allow-list only: express denial by omitting grants)");
        }
    }
    if let Some(net) = &manifest.network {
        if net.mode == NetworkMode::Proxy {
            unsupported.push("network.mode: proxy (needs nono's supervising proxy process)");
        }
        if !net.allow_domains.is_empty() {
            unsupported.push("network.allow_domains (domain filtering happens in nono's proxy)");
        }
        if !net.endpoints.is_empty() {
            unsupported.push("network.endpoints (L7 filtering happens in nono's proxy)");
        }
        if !net.dns {
            unsupported.push("network.dns: false (DNS interception happens in nono's proxy)");
        }
    }
    if !manifest.credentials.is_empty() {
        unsupported.push("credentials (injection happens in nono's proxy)");
    }
    if manifest.rollback.as_ref().is_some_and(|r| r.enabled) {
        unsupported.push("rollback.enabled (snapshots need nono's supervising parent)");
    }
    if manifest
        .process
        .as_ref()
        .is_some_and(|p| p.exec_strategy == ExecStrategy::Supervised)
    {
        unsupported.push("process.exec_strategy: supervised (this server applies the sandbox in-process)");
    }

    if unsupported.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "--sandbox-manifest: the following manifest options need nono's CLI \
             supervisor and cannot be enforced by this server's in-process \
             sandbox; refusing to start with them silently ignored:\n  - {}",
            unsupported.join("\n  - ")
        )
    }
}

/// Apply the OS sandbox described by the manifest file.
///
/// Must be called while the process is still single-threaded: Landlock
/// confines the calling thread and everything it spawns afterwards, so the
/// tokio runtime and the V8 platform must not exist yet. `main()` enforces
/// this ordering.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn apply(manifest_path: &str) -> Result<()> {
    use nono::manifest::CapabilityManifest;
    use nono::{CapabilitySet, Sandbox};

    let json = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("--sandbox-manifest: failed to read {manifest_path}"))?;
    let manifest = CapabilityManifest::from_json(&json)
        .map_err(|e| anyhow::anyhow!("--sandbox-manifest {manifest_path}: {e}"))?;
    reject_supervisor_features(&manifest)?;

    let support = Sandbox::support_info();
    if !support.is_supported {
        anyhow::bail!(
            "--sandbox-manifest requested but this system cannot enforce it ({}): {}. \
             Refusing to run unconfined; drop --sandbox-manifest to accept that.",
            support.platform,
            support.details
        );
    }

    let caps = CapabilitySet::try_from(&manifest)
        .map_err(|e| anyhow::anyhow!("--sandbox-manifest {manifest_path}: {e}"))?;

    #[cfg(target_os = "linux")]
    {
        let fallback = Sandbox::apply(&caps)
            .map_err(|e| anyhow::anyhow!("--sandbox-manifest: failed to apply sandbox: {e}"))?;
        if let nono::sandbox::SeccompNetFallback::ProxyOnly { .. } = fallback {
            // Unreachable: proxy mode is rejected above, and the other
            // variants (None / inline BlockAll) need no follow-up.
            anyhow::bail!("--sandbox-manifest: unexpected proxy-only seccomp fallback");
        }
    }
    #[cfg(target_os = "macos")]
    Sandbox::apply(&caps)
        .map_err(|e| anyhow::anyhow!("--sandbox-manifest: failed to apply sandbox: {e}"))?;

    tracing::info!(
        "OS sandbox applied ({}) from manifest {}",
        support.platform,
        manifest_path,
    );
    Ok(())
}

/// Stub for platforms without an OS sandbox backend: fail closed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply(_manifest_path: &str) -> Result<()> {
    anyhow::bail!(
        "--sandbox-manifest is only supported on Linux (Landlock) and macOS \
         (Seatbelt); refusing to run unconfined on this platform. Drop \
         --sandbox-manifest to accept that."
    );
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use nono::manifest::CapabilityManifest;

    fn manifest(json: &str) -> CapabilityManifest {
        CapabilityManifest::from_json(json).expect("valid manifest JSON")
    }

    #[test]
    fn minimal_manifest_is_accepted_and_converts() {
        // nono resolves grant paths at conversion time, so they must exist.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("policy.rego");
        std::fs::write(&file, "package x").unwrap();
        let m = manifest(&format!(
            r#"{{
                "version": "0.1.0",
                "filesystem": {{"grants": [
                    {{"path": "{}", "access": "readwrite"}},
                    {{"path": "{}", "access": "read", "type": "file"}}
                ]}},
                "network": {{"mode": "blocked", "ports": {{"bind": [8080]}}}}
            }}"#,
            dir.path().display(),
            file.display(),
        ));
        reject_supervisor_features(&m).expect("in-process subset should pass");
        nono::CapabilitySet::try_from(&m).expect("nono conversion should succeed");
    }

    #[test]
    fn supervisor_only_features_are_rejected_loudly() {
        for (json, needle) in [
            (r#"{"version":"0.1.0","network":{"mode":"proxy"}}"#, "network.mode: proxy"),
            (
                r#"{"version":"0.1.0","network":{"allow_domains":[".example.com"]}}"#,
                "allow_domains",
            ),
            (
                r#"{"version":"0.1.0","network":{"endpoints":[{"host":"api.example.com"}]}}"#,
                "endpoints",
            ),
            (r#"{"version":"0.1.0","network":{"dns":false}}"#, "dns"),
            (
                r#"{"version":"0.1.0","credentials":[{"name":"t","upstream":"api.example.com","source":"env://TOKEN"}]}"#,
                "credentials",
            ),
            (
                r#"{"version":"0.1.0","filesystem":{"grants":[],"deny":[{"path":"/etc/shadow"}]}}"#,
                "filesystem.deny",
            ),
            (
                r#"{"version":"0.1.0","process":{"exec_strategy":"supervised"}}"#,
                "supervised",
            ),
        ] {
            let err = reject_supervisor_features(&manifest(json))
                .expect_err(&format!("expected rejection for {json}"));
            assert!(
                err.to_string().contains(needle),
                "error for {json} should mention {needle}, got: {err}"
            );
        }
    }

    #[test]
    fn rollback_disabled_is_fine_enabled_is_not() {
        let ok = manifest(r#"{"version":"0.1.0","rollback":{"enabled":false}}"#);
        reject_supervisor_features(&ok).expect("disabled rollback is a no-op");

        // rollback.enabled also fails nono's own validate() (it requires
        // exec_strategy supervised), so assert on our earlier, clearer error.
        let bad = manifest(
            r#"{"version":"0.1.0","rollback":{"enabled":true},"process":{"exec_strategy":"supervised"}}"#,
        );
        let err = reject_supervisor_features(&bad).expect_err("enabled rollback must be rejected");
        assert!(err.to_string().contains("rollback"));
    }
}
