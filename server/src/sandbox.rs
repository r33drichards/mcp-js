//! OS-level sandbox layer (`--sandbox`), powered by the [nono] crate.
//!
//! When enabled, the whole server process is confined by the operating
//! system — Landlock on Linux (kernel 5.13+), Seatbelt on macOS — before the
//! tokio runtime or the V8 platform spawn a single thread. The capability set
//! is derived from the parsed CLI configuration: storage directories get
//! read-write, configuration/policy/WASM files get read-only, system paths get
//! read-only, and outbound network access is granted only when a configured
//! feature needs it (see [`SandboxPlan`]).
//!
//! This layer sits *underneath* the OPA policy layer: policies decide what JS
//! is allowed to ask for; the OS sandbox bounds what the process can do even
//! if V8, the policy chain, or the server itself is compromised. Applying the
//! sandbox is irreversible for the lifetime of the process, and every thread
//! and child process created afterwards inherits it.
//!
//! Fail-closed contract: if `--sandbox` is requested and the platform cannot
//! enforce it (unsupported OS, Landlock disabled, a policy the kernel cannot
//! express), startup aborts rather than running unconfined.
//!
//! [nono]: https://github.com/nolabs-ai/nono

use crate::cli::{Cli, SandboxNetwork, StoreKind};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Default heap directory when `--heap-store dir` is set without `--heap-dir`.
/// Must match the fallback in `main.rs`.
pub const DEFAULT_HEAP_DIR: &str = "/tmp/mcp-v8-heaps";

/// Everything the sandbox will grant, derived from the CLI configuration.
///
/// Derivation is pure (no syscalls beyond path existence checks in
/// [`SandboxPlan::from_cli`]'s caller) so it can be unit-tested without
/// actually confining the test process.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SandboxPlan {
    /// Directories granted recursive read-write (created before applying).
    pub write_dirs: BTreeSet<PathBuf>,
    /// Paths granted recursive read-only access (files or directories).
    pub read_paths: BTreeSet<PathBuf>,
    /// Whether outbound TCP stays open.
    pub allow_outbound: bool,
    /// Listener ports that must remain bindable when outbound is blocked.
    pub bind_ports: Vec<u16>,
}

impl SandboxPlan {
    /// Derive the capability plan from the parsed CLI.
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
        for path in &cli.sandbox_allow_write {
            plan.write_dirs.insert(PathBuf::from(path));
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
        for path in &cli.sandbox_allow_read {
            plan.read_paths.insert(PathBuf::from(path));
        }

        // ── Network ────────────────────────────────────────────────────────
        plan.bind_ports = [cli.http_port, cli.sse_port, cli.cluster_port]
            .into_iter()
            .flatten()
            .collect();
        plan.allow_outbound = match cli.sandbox_network {
            SandboxNetwork::Allow => true,
            SandboxNetwork::Block => false,
            SandboxNetwork::Auto => needs_outbound(cli),
        };

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

/// Whether any configured feature needs outbound network access
/// (`--sandbox-network auto`).
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

/// Apply the OS sandbox for the given CLI configuration.
///
/// Must be called while the process is still single-threaded: Landlock
/// confines the calling thread and everything it spawns afterwards, so the
/// tokio runtime and the V8 platform must not exist yet. `main()` enforces
/// this ordering.
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
        anyhow::anyhow!("--sandbox: cannot grant {mode} on {}: {e}", path.display())
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn apply(cli: &Cli) -> Result<()> {
    use nono::{AccessMode, CapabilitySet, Sandbox};

    let support = Sandbox::support_info();
    if !support.is_supported {
        anyhow::bail!(
            "--sandbox requested but this system cannot enforce it ({}): {}. \
             Refusing to run unconfined; drop --sandbox to accept that.",
            support.platform,
            support.details
        );
    }

    let plan = SandboxPlan::from_cli(cli);

    // Storage directories must exist to be granted (Landlock rules attach to
    // real inodes); the server would create them later anyway.
    for dir in &plan.write_dirs {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("--sandbox: failed to create writable dir {}", dir.display()))?;
    }

    let mut caps = CapabilitySet::new();

    for dir in &plan.write_dirs {
        caps = grant(caps, dir, AccessMode::ReadWrite)?;
    }
    for path in &plan.read_paths {
        if !path.exists() {
            anyhow::bail!(
                "--sandbox: configured path {} does not exist; the sandbox would \
                 silently deny it at runtime",
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
    if plan.allow_outbound && cli.s3_bucket.is_some() {
        if let Some(home) = std::env::var_os("HOME") {
            let aws = Path::new(&home).join(".aws");
            if aws.is_dir() {
                caps = grant(caps, &aws, AccessMode::Read)?;
            }
        }
    }

    if plan.allow_outbound {
        // NetworkMode::AllowAll (the default): no TCP restrictions, listeners
        // and egress both work everywhere.
        tracing::info!("OS sandbox: outbound network ALLOWED");
    } else {
        caps = caps.block_network();
        for port in &plan.bind_ports {
            caps = caps.allow_tcp_bind(*port);
        }
        tracing::info!(
            "OS sandbox: outbound network BLOCKED{}",
            if plan.bind_ports.is_empty() {
                String::new()
            } else {
                format!(" (listeners may bind {:?})", plan.bind_ports)
            }
        );
    }

    // Port-level bind exceptions need Landlock's TCP rules (ABI v4, Linux
    // 6.7+). On older kernels nono's seccomp fallback is all-or-nothing and
    // silently skips this mix — fail closed instead.
    #[cfg(target_os = "linux")]
    {
        let abi = Sandbox::detect_abi().map_err(|e| {
            anyhow::anyhow!(
                "--sandbox: Landlock unavailable ({e}); refusing to run unconfined. \
                 Landlock requires Linux 5.13+ with CONFIG_SECURITY_LANDLOCK and \
                 landlock in the lsm= boot parameter."
            )
        })?;
        if !plan.allow_outbound && !plan.bind_ports.is_empty() && !abi.has_network() {
            anyhow::bail!(
                "--sandbox: blocking outbound network while serving on ports {:?} \
                 needs Landlock TCP rules (ABI v4, Linux 6.7+); this kernel only \
                 offers all-or-nothing network filtering. Use --sandbox-network \
                 allow, drop the HTTP/SSE listener (stdio transport), or upgrade \
                 the kernel.",
                plan.bind_ports
            );
        }
        let fallback = Sandbox::apply_with_abi(&caps, &abi)
            .map_err(|e| anyhow::anyhow!("--sandbox: failed to apply sandbox: {e}"))?;
        if let nono::sandbox::SeccompNetFallback::ProxyOnly { .. } = fallback {
            // Unreachable: this server never configures ProxyOnly mode, and
            // the other variants (None / inline BlockAll) need no follow-up.
            anyhow::bail!("--sandbox: unexpected proxy-only seccomp fallback");
        }
    }
    #[cfg(target_os = "macos")]
    Sandbox::apply(&caps).map_err(|e| anyhow::anyhow!("--sandbox: failed to apply sandbox: {e}"))?;

    tracing::info!(
        "OS sandbox applied ({}): {} read-write dir(s), {} read-only grant(s)",
        support.platform,
        plan.write_dirs.len(),
        plan.read_paths.len(),
    );
    Ok(())
}

/// Stub for platforms without an OS sandbox backend: `--sandbox` fails closed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply(_cli: &Cli) -> Result<()> {
    anyhow::bail!(
        "--sandbox is only supported on Linux (Landlock) and macOS (Seatbelt); \
         refusing to run unconfined on this platform. Drop --sandbox to accept that."
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
    fn default_plan_is_minimal_and_offline() {
        let cli = parse(&[]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.write_dirs.contains(Path::new("/tmp/mcp-v8-sessions")));
        assert_eq!(plan.write_dirs.len(), 1);
        assert!(plan.read_paths.is_empty());
        assert!(!plan.allow_outbound, "nothing configured needs egress");
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
        assert!(!plan.allow_outbound);
    }

    #[test]
    fn explicit_grants_and_listeners_are_carried() {
        let cli = parse(&[
            "--http-port", "8080",
            "--sandbox-allow-read", "/srv/scripts",
            "--sandbox-allow-write", "/srv/out",
        ]);
        let plan = SandboxPlan::from_cli(&cli);
        assert!(plan.read_paths.contains(Path::new("/srv/scripts")));
        assert!(plan.write_dirs.contains(Path::new("/srv/out")));
        assert_eq!(plan.bind_ports, vec![8080]);
        assert!(!plan.allow_outbound, "an HTTP listener alone needs no egress");
    }

    #[test]
    fn auto_network_opens_for_s3_jwks_cluster_policies() {
        for args in [
            vec!["--heap-store", "s3", "--s3-bucket", "b"],
            vec!["--jwks-url", "https://idp/certs"],
            vec!["--cluster-port", "4000"],
            vec!["--policies-json", r#"{"fetch":{"policies":[]}}"#],
            vec!["--allow-external-modules"],
        ] {
            let cli = parse(&args);
            assert!(
                SandboxPlan::from_cli(&cli).allow_outbound,
                "expected outbound for {args:?}"
            );
        }
    }

    #[test]
    fn network_override_beats_auto() {
        let cli = parse(&["--jwks-url", "https://idp/certs", "--sandbox-network", "block"]);
        assert!(!SandboxPlan::from_cli(&cli).allow_outbound);
        let cli = parse(&["--sandbox-network", "allow"]);
        assert!(SandboxPlan::from_cli(&cli).allow_outbound);
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
        assert!(plan.allow_outbound);
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
}
