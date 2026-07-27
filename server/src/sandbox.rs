//! OS-level sandbox layer (`--sandbox-manifest`), powered by the [nono] crate.
//!
//! The flag takes a nono capability manifest — the JSON format defined by
//! nono's `capability-manifest` schema — and **composes** it with the
//! capabilities the server needs to function. nono's capability model is
//! additive (grants are an allow-list; duplicates deduplicate), so the final
//! capability set is the union of two layers:
//!
//! 1. **The manifest**, converted verbatim through nono's own
//!    `CapabilityManifest -> CapabilitySet` conversion: extra filesystem
//!    grants (e.g. `run_js` script roots), the network mode, and port
//!    allowlists, all in nono's schema and semantics.
//! 2. **The server baseline**, derived from the parsed CLI configuration
//!    (see [`SandboxPlan`]): storage directories read-write,
//!    configuration/policy/WASM inputs read-only, system library paths
//!    read-only, and bind grants for the configured listener ports.
//!
//! The manifest never needs to restate what the server already knows about
//! itself — a minimal `{"version": "0.1.0"}` manifest yields a confined
//! server with baseline grants and nono's default (unrestricted) network.
//! The one thing the baseline never does is widen *egress*: outbound network
//! posture is exactly what the manifest says, so `"network": {"mode":
//! "blocked"}` holds even when a configured feature (S3, JWKS, fetch
//! policies) would want to dial out — the server warns and starts anyway.
//!
//! The sandbox is applied before the tokio runtime or the V8 platform spawn
//! a single thread (Landlock confines the calling thread and everything it
//! spawns afterwards), and is irreversible for the lifetime of the process.
//! It sits *underneath* the OPA policy layer: policies decide what JS may
//! ask for; the OS sandbox bounds what the process can do even if V8, the
//! policy chain, or the server itself is compromised.
//!
//! Fail-closed contract, in two parts:
//! - If the platform cannot enforce the composed capability set (unsupported
//!   OS, Landlock disabled), startup aborts rather than running unconfined.
//! - Manifest features that only work under nono's CLI supervisor — network
//!   `proxy` mode (and its `allow_domains`/`endpoints` filtering), `dns:
//!   false`, `credentials`, `rollback`, filesystem `deny` rules,
//!   `exec_strategy: supervised` — are rejected at startup rather than
//!   silently ignored: this server applies the sandbox in-process, where
//!   only kernel-expressible rules (path grants, port allowlists, block-all)
//!   exist.
//!
//! [nono]: https://github.com/nolabs-ai/nono

use crate::cli::{Cli, StoreKind};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Default heap directory when `--heap-store dir` is set without `--heap-dir`.
/// Must match the fallback in `main.rs`.
pub const DEFAULT_HEAP_DIR: &str = "/tmp/mcp-v8-heaps";

/// The server-baseline layer: everything the process needs to function,
/// derived from the CLI configuration and composed underneath the
/// user-supplied manifest.
///
/// Derivation is pure (no syscalls beyond path existence checks in the
/// caller) so it can be unit-tested without confining the test process.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SandboxPlan {
    /// Directories granted recursive read-write (created before applying).
    pub write_dirs: BTreeSet<PathBuf>,
    /// Paths granted recursive read-only access (files or directories).
    pub read_paths: BTreeSet<PathBuf>,
    /// Listener ports that must remain bindable when the manifest blocks
    /// the network.
    pub bind_ports: Vec<u16>,
}

impl SandboxPlan {
    /// Derive the baseline capability plan from the parsed CLI.
    pub fn from_cli(cli: &Cli) -> Self {
        let mut plan = SandboxPlan::default();

        // ── Read-write: every node-local store the server opens ────────────
        // The session db is unconditional (session log, heap tags, execution
        // registry, cluster db all live under it).
        plan.write_dirs.insert(PathBuf::from(&cli.session_db_path));
        if cli.heap_store == StoreKind::Dir {
            let dir = cli.heap_dir.clone().unwrap_or_else(|| DEFAULT_HEAP_DIR.to_string());
            plan.write_dirs.insert(PathBuf::from(dir));
        }
        if cli.fs_store == StoreKind::Dir {
            let dir = cli
                .fs_dir
                .clone()
                .unwrap_or_else(|| format!("{}/fs-blobs", cli.session_db_path));
            plan.write_dirs.insert(PathBuf::from(dir));
        }
        if cli.fs_enabled() {
            let labels = cli
                .fs_labels_db
                .clone()
                .unwrap_or_else(|| format!("{}/fs-labels", cli.session_db_path));
            plan.write_dirs.insert(PathBuf::from(labels));
        }
        if let Some(cache) = &cli.cache_dir {
            plan.write_dirs.insert(PathBuf::from(cache));
        }

        // ── Read-only: configuration inputs resolved after the sandbox ─────
        // The --config file itself is parsed before apply, but flags that
        // point at files (@file prompts, WASM modules, JSON configs, local
        // Rego policies) are read inside async_main, i.e. post-confinement.
        if let Some(config) = &cli.config {
            plan.read_paths.insert(PathBuf::from(config));
        }
        for value in [&cli.instructions, &cli.run_js_description].into_iter().flatten() {
            // Mirrors resolve_text_or_file: "@path" reads a file, "@@" is a
            // literal-@ escape, anything else is inline text.
            if let Some(path) = value.strip_prefix('@') {
                if !path.starts_with('@') {
                    plan.read_paths.insert(PathBuf::from(path));
                }
            }
        }
        for entry in &cli.wasm_modules {
            if let Some((_name, rest)) = entry.split_once('=') {
                plan.read_paths.insert(wasm_entry_path(rest));
            }
        }
        for opt in [&cli.wasm_config, &cli.fetch_header_config, &cli.mcp_config, &cli.policies_json] {
            if let Some(value) = opt {
                // Each of these accepts a file path or inline JSON; inline
                // JSON never names an existing file.
                if Path::new(value).is_file() {
                    plan.read_paths.insert(PathBuf::from(value));
                }
            }
        }
        // Local Rego policies referenced from the policies config as
        // file:// URLs are loaded (and may be reloaded) after confinement.
        if let Some(policies) = &cli.policies_json {
            let json = if Path::new(policies).is_file() {
                std::fs::read_to_string(policies).ok()
            } else {
                Some(policies.clone())
            };
            if let Some(json) = json {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                    collect_file_urls(&value, &mut plan.read_paths);
                }
            }
        }

        // ── Listeners ──────────────────────────────────────────────────────
        plan.bind_ports = [cli.http_port, cli.sse_port, cli.cluster_port]
            .into_iter()
            .flatten()
            .collect();

        plan
    }
}

/// Extract the file path from the `--wasm-module` remainder
/// (`/path/to.wasm[:max_memory]`), mirroring `load_wasm_modules`: the suffix
/// after the last `:` is a size only if it parses as one, otherwise the whole
/// remainder is the path.
fn wasm_entry_path(rest: &str) -> PathBuf {
    if let Some((path, suffix)) = rest.rsplit_once(':') {
        let is_size = {
            let trimmed = suffix.trim();
            let digits = trimmed
                .strip_suffix(['k', 'K', 'm', 'M', 'g', 'G'])
                .unwrap_or(trimmed);
            !digits.is_empty() && digits.parse::<usize>().is_ok()
        };
        if is_size {
            return PathBuf::from(path);
        }
    }
    PathBuf::from(rest)
}

/// Recursively collect `file://` URL targets from a policies JSON value.
fn collect_file_urls(value: &serde_json::Value, out: &mut BTreeSet<PathBuf>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(path) = s.strip_prefix("file://") {
                out.insert(PathBuf::from(path));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_file_urls(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_file_urls(item, out);
            }
        }
        _ => {}
    }
}

/// Whether any configured feature wants outbound network access. Used only
/// to *warn* when the manifest blocks the network anyway — the manifest owns
/// the egress posture.
fn needs_outbound(cli: &Cli) -> bool {
    cli.heap_store == StoreKind::S3
        || cli.fs_store == StoreKind::S3
        || cli.s3_bucket.is_some()
        || cli.jwks_url.is_some()
        || cli.cluster_port.is_some()
        || cli.join.is_some()
        || !cli.peers.is_empty()
        // A policies config can enable JS fetch() and can point at remote OPA
        // servers; either way the process needs egress.
        || cli.policies_json.is_some()
        // Fetch header injection only matters if fetch is reachable, and the
        // OAuth form dials a token endpoint itself.
        || !cli.fetch_headers.is_empty()
        || cli.fetch_header_config.is_some()
        // SSE MCP server modules dial out; stdio ones only spawn children.
        || cli.mcp_servers.iter().any(|entry| entry.contains("=sse:"))
        || cli.mcp_config.is_some()
        || cli.allow_external_modules
}

/// System paths every confined server needs read (and execute, for spawning
/// stdio MCP servers / policy-gated subprocesses): shared libraries,
/// interpreters, TLS roots, resolver config. Paths that do not exist on the
/// host are skipped.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn system_read_paths() -> Vec<&'static str> {
    #[cfg(target_os = "linux")]
    {
        // /proc/self canonicalizes to this process's /proc/<pid> at grant
        // time, so the grant is scoped to our own procfs entry (V8 and glibc
        // probe maps/status there), not to other processes'.
        vec![
            "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/opt", "/nix",
            "/proc/self",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "/usr",
            "/bin",
            "/sbin",
            "/opt",
            "/System",
            "/Library",
            "/private/etc",
            "/private/var/db",
            "/nix",
        ]
    }
}

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

/// Grant `path` (auto-detecting file vs directory) at `mode`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn grant(
    caps: nono::CapabilitySet,
    path: &Path,
    mode: nono::AccessMode,
) -> Result<nono::CapabilitySet> {
    let granted = if path.is_dir() {
        caps.allow_path(path, mode)
    } else {
        caps.allow_file(path, mode)
    };
    granted.map_err(|e| {
        anyhow::anyhow!("--sandbox-manifest: cannot grant {mode} on {}: {e}", path.display())
    })
}

/// Apply the OS sandbox: the user manifest composed with the server baseline.
///
/// Must be called while the process is still single-threaded: Landlock
/// confines the calling thread and everything it spawns afterwards, so the
/// tokio runtime and the V8 platform must not exist yet. `main()` enforces
/// this ordering.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn apply(cli: &Cli) -> Result<()> {
    use nono::manifest::{CapabilityManifest, NetworkMode};
    use nono::{AccessMode, CapabilitySet, Sandbox};

    let Some(manifest_path) = &cli.sandbox_manifest else {
        return Ok(());
    };

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

    // Layer 1: the manifest, verbatim, through nono's own conversion.
    let mut caps = CapabilitySet::try_from(&manifest)
        .map_err(|e| anyhow::anyhow!("--sandbox-manifest {manifest_path}: {e}"))?;

    // Layer 2: the server baseline. Grants are additive and deduplicated by
    // nono, so composing on top of the manifest cannot narrow it.
    let plan = SandboxPlan::from_cli(cli);

    // Storage directories must exist to be granted (kernel rules attach to
    // real inodes); the server would create them later anyway.
    for dir in &plan.write_dirs {
        std::fs::create_dir_all(dir).with_context(|| {
            format!("--sandbox-manifest: failed to create writable dir {}", dir.display())
        })?;
        caps = grant(caps, dir, AccessMode::ReadWrite)?;
    }
    for path in &plan.read_paths {
        if !path.exists() {
            anyhow::bail!(
                "--sandbox-manifest: configured path {} does not exist; the sandbox \
                 would silently deny it at runtime",
                path.display()
            );
        }
        caps = grant(caps, path, AccessMode::Read)?;
    }
    for path in system_read_paths() {
        let path = Path::new(path);
        if path.exists() {
            caps = grant(caps, path, AccessMode::Read)?;
        }
    }
    // Device nodes the runtime touches: null sink and entropy sources.
    for dev in ["/dev/null", "/dev/urandom", "/dev/random", "/dev/zero"] {
        let path = Path::new(dev);
        if path.exists() {
            let mode = if dev == "/dev/null" { AccessMode::ReadWrite } else { AccessMode::Read };
            caps = grant(caps, path, mode)?;
        }
    }
    // AWS SDK credential/config files, only when an S3 backend is configured.
    if cli.s3_bucket.is_some() {
        if let Some(home) = std::env::var_os("HOME") {
            let aws = Path::new(&home).join(".aws");
            if aws.is_dir() {
                caps = grant(caps, &aws, AccessMode::Read)?;
            }
        }
    }

    // Network: the manifest owns the egress posture; the baseline only adds
    // bind grants for listeners the configuration says must serve.
    let blocked = manifest
        .network
        .as_ref()
        .is_some_and(|net| net.mode == NetworkMode::Blocked);
    if blocked {
        for port in &plan.bind_ports {
            caps = caps.allow_tcp_bind(*port);
        }
        if needs_outbound(cli) {
            tracing::warn!(
                "--sandbox-manifest blocks the network, but the configuration \
                 enables features that dial out (S3/JWKS/cluster/policies/SSE \
                 MCP/external modules); those will fail at runtime"
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        let abi = Sandbox::detect_abi().map_err(|e| {
            anyhow::anyhow!(
                "--sandbox-manifest: Landlock unavailable ({e}); refusing to run \
                 unconfined. Landlock requires Linux 5.13+ with \
                 CONFIG_SECURITY_LANDLOCK and landlock in the lsm= boot parameter."
            )
        })?;
        if blocked && !plan.bind_ports.is_empty() && !abi.has_network() {
            anyhow::bail!(
                "--sandbox-manifest: blocking the network while serving on ports \
                 {:?} needs Landlock TCP rules (ABI v4, Linux 6.7+); this kernel \
                 only offers all-or-nothing network filtering. Use an \
                 unrestricted network mode, drop the HTTP/SSE listener (stdio \
                 transport), or upgrade the kernel.",
                plan.bind_ports
            );
        }
        let fallback = Sandbox::apply_with_abi(&caps, &abi)
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
        "OS sandbox applied ({}): manifest {} composed with server baseline \
         ({} read-write dir(s), {} read-only grant(s))",
        support.platform,
        manifest_path,
        plan.write_dirs.len(),
        plan.read_paths.len(),
    );
    Ok(())
}

/// Stub for platforms without an OS sandbox backend: fail closed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply(cli: &Cli) -> Result<()> {
    if cli.sandbox_manifest.is_none() {
        return Ok(());
    }
    anyhow::bail!(
        "--sandbox-manifest is only supported on Linux (Landlock) and macOS \
         (Seatbelt); refusing to run unconfined on this platform. Drop \
         --sandbox-manifest to accept that."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        let mut argv = vec!["server"];
        argv.extend_from_slice(args);
        Cli::parse_from(argv)
    }

    #[test]
    fn default_baseline_is_minimal() {
        let cli = parse(&[]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.write_dirs.contains(Path::new("/tmp/mcp-v8-sessions")));
        assert_eq!(plan.write_dirs.len(), 1);
        assert!(plan.read_paths.is_empty());
        assert!(plan.bind_ports.is_empty());
    }

    #[test]
    fn dir_stores_and_cache_become_writable() {
        let cli = parse(&[
            "--heap-store", "dir",
            "--fs-store", "dir",
            "--session-db-path", "/var/lib/v8/sessions",
        ]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.write_dirs.contains(Path::new(DEFAULT_HEAP_DIR)));
        assert!(plan.write_dirs.contains(Path::new("/var/lib/v8/sessions")));
        assert!(plan.write_dirs.contains(Path::new("/var/lib/v8/sessions/fs-blobs")));
        assert!(plan.write_dirs.contains(Path::new("/var/lib/v8/sessions/fs-labels")));
    }

    #[test]
    fn listener_ports_are_carried() {
        let cli = parse(&["--http-port", "8080", "--cluster-port", "4000"]);
        assert_eq!(SandboxPlan::from_cli(&cli).bind_ports, vec![8080, 4000]);
    }

    #[test]
    fn policy_file_urls_get_read_grants() {
        let cli = parse(&[
            "--policies-json",
            r#"{"fetch":{"policies":[{"url":"file:///etc/mcp-v8/fetch.rego"}]},
                "modules":{"policies":[{"url":"https://opa.internal/v1"}]}}"#,
        ]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.read_paths.contains(Path::new("/etc/mcp-v8/fetch.rego")));
        assert!(needs_outbound(&cli), "remote OPA should trip the egress warning");
    }

    #[test]
    fn at_file_prompts_get_read_grants() {
        let cli = parse(&[
            "--instructions", "@/etc/mcp-v8/prompt.txt",
            "--run-js-description", "@@literal-not-a-file",
        ]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.read_paths.contains(Path::new("/etc/mcp-v8/prompt.txt")));
        assert_eq!(plan.read_paths.len(), 1, "@@ escape is inline text, not a path");
    }

    #[test]
    fn wasm_module_paths_get_read_grants() {
        let cli = parse(&[
            "--wasm-module", "math=/opt/mods/math.wasm:16m",
            "--wasm-module", "raw=/opt/mods/with:colon.wasm",
        ]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.read_paths.contains(Path::new("/opt/mods/math.wasm")));
        assert!(plan.read_paths.contains(Path::new("/opt/mods/with:colon.wasm")));
    }

    #[test]
    fn egress_warning_trips_for_outbound_features() {
        for args in [
            vec!["--heap-store", "s3", "--s3-bucket", "b"],
            vec!["--jwks-url", "https://idp/certs"],
            vec!["--cluster-port", "4000"],
            vec!["--allow-external-modules"],
        ] {
            let cli = parse(&args);
            assert!(needs_outbound(&cli), "expected outbound for {args:?}");
        }
        assert!(!needs_outbound(&parse(&[])), "default config needs no egress");
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod manifest_tests {
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
