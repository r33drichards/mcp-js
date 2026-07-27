# OS sandboxing (--sandbox)

`--sandbox` confines the whole server process with an OS-enforced sandbox —
[Landlock](https://landlock.io/) on Linux (kernel 5.13+), Seatbelt on macOS —
using the [nono](https://github.com/nolabs-ai/nono) sandboxing library. It is
defense in depth *underneath* the [OPA/Rego policy layer](policies.md): even if
V8, the policy chain, or the server itself were compromised, the kernel still
refuses filesystem and network access that was never granted.

See [Concepts: OS sandboxing](../concepts/os-sandbox.md) for the security
model. This page shows how to turn it on and adapt it.

## Turn it on

```bash
mcp-v8 --http-port 8080 --heap-store dir --heap-dir /var/lib/mcp-v8/heaps --sandbox
```

Nothing else is required. At startup — before the async runtime or V8 spawn a
single thread — the server derives a capability set from the rest of its
configuration and asks the kernel to enforce it:

- **Read-write**: the session db (`--session-db-path`), heap dir
  (`--heap-dir`), fs blob store (`--fs-dir`), fs label db (`--fs-labels-db`),
  and S3 cache (`--cache-dir`) — whichever are in play, created if missing.
- **Read-only**: the `--config` file, `@file` prompt overrides, WASM modules,
  the JSON files behind `--wasm-config` / `--mcp-config` /
  `--fetch-header-config` / `--policies-json`, and any local Rego policies
  referenced from the policies config as `file://` URLs. Plus system paths
  (`/usr`, `/lib`, `/etc`, …) so shared libraries load and child processes can
  spawn.
- **Network**: derived by `--sandbox-network auto` (see below).

Once applied the sandbox is irreversible for the life of the process, and
every thread and child process inherits it.

## Fail-closed contract

If the platform cannot enforce the sandbox — an unsupported OS, a kernel
without Landlock, a policy the kernel cannot express — startup **aborts** with
an explanatory error. `--sandbox` never degrades to running unconfined.

```
Error: --sandbox requested but this system cannot enforce it (linux):
Landlock not available. Requires Linux kernel 5.13+ with Landlock enabled.
```

## Grant extra paths

The derivation only sees paths named in the configuration. Grant anything else
explicitly:

```bash
mcp-v8 --sandbox \
  --allow-run-js-file \
  --sandbox-allow-read /srv/scripts \        # scripts run via run_js file=...
  --sandbox-allow-write /srv/output          # extra writable tree
```

Both flags take files or directories (directory grants are recursive) and can
be repeated. Typical cases:

- `--allow-run-js-file` or a `run_js_file` policy: grant the script roots with
  `--sandbox-allow-read`.
- `--fs-passthrough`: grant the read-only lower-layer roots (e.g.
  `/opt/languages`) with `--sandbox-allow-read`.
- Subprocess policies that run tools outside the system paths: grant the tool
  and whatever it reads/writes.

In a `--config` file the flags are keys like everything else:

```toml
sandbox = true
sandbox_allow_read = ["/srv/scripts"]
sandbox_network = "block"
```

## Control the network posture

`--sandbox-network` takes three values:

- `auto` (default) — outbound TCP stays open only when a configured feature
  needs it: S3 stores, `--jwks-url`, clustering, `--policies-json` (which can
  enable JS `fetch()` and remote OPA), fetch-header injection, SSE MCP
  servers, or `--allow-external-modules`. Otherwise outbound is blocked.
- `allow` — leave the network unrestricted.
- `block` — force outbound closed even if features appear to need it.

Configured `--http-port` / `--sse-port` / `--cluster-port` listeners are
always allowed to bind, even under `block`. Port-level exceptions need
Landlock ABI v4 (Linux 6.7+); on older kernels network filtering is
all-or-nothing, and the server aborts at startup if the requested mix cannot
be expressed (use `--sandbox-network allow`, the stdio transport, or a newer
kernel).

A hardened, fully offline stdio server:

```bash
mcp-v8 --heap-store dir --heap-dir /var/lib/mcp-v8/heaps \
  --sandbox --sandbox-network block \
  --harden-freeze-ops --harden-remove-bootstrap
```

## Troubleshooting

Denials surface as `EACCES`/`Permission denied` errors from whatever tried
the access (a `run_js file=...` read, a subprocess, `fetch`). When something
legitimate is denied:

1. Find the path (or endpoint) in the error message.
2. Grant it with `--sandbox-allow-read` / `--sandbox-allow-write`, or open the
   network with `--sandbox-network allow`.
3. Restart — grants cannot be widened at runtime by design.

`RUST_LOG=info` logs the derived grants at startup
(`OS sandbox applied (linux): N read-write dir(s), M read-only grant(s)`).
