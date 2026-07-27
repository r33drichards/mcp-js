# OS sandboxing (--sandbox-manifest)

`--sandbox-manifest <file.json>` confines the whole server process with an
OS-enforced sandbox — [Landlock](https://landlock.io/) on Linux (kernel
5.13+), Seatbelt on macOS — described by a
[nono](https://github.com/nolabs-ai/nono) **capability manifest**. The
manifest is nono's own JSON format and is passed to nono verbatim: mcp-v8
adds no flags, derivation, or defaults on top. It is defense in depth
*underneath* the [OPA/Rego policy layer](policies.md): even if V8, the policy
chain, or the server itself were compromised, the kernel still refuses
filesystem and network access the manifest never granted.

See [Concepts: OS sandboxing](../concepts/os-sandbox.md) for the security
model. This page shows how to write a manifest for mcp-v8.

## A working manifest

The manifest must grant **everything the process needs** — including the
server's own storage directories and the system paths shared libraries load
from. A hardened, fully offline stdio server with a directory heap store:

```json
{
  "version": "0.1.0",
  "filesystem": {
    "grants": [
      { "path": "/var/lib/mcp-v8/sessions", "access": "readwrite" },
      { "path": "/var/lib/mcp-v8/heaps", "access": "readwrite" },

      { "path": "/usr", "access": "read" },
      { "path": "/bin", "access": "read" },
      { "path": "/lib", "access": "read" },
      { "path": "/lib64", "access": "read" },
      { "path": "/etc", "access": "read" },
      { "path": "/proc/self", "access": "read" },
      { "path": "/dev/null", "access": "readwrite" },
      { "path": "/dev/urandom", "access": "read" }
    ]
  },
  "network": { "mode": "blocked" }
}
```

```bash
mcp-v8 --heap-store dir --heap-dir /var/lib/mcp-v8/heaps \
  --session-db-path /var/lib/mcp-v8/sessions \
  --sandbox-manifest /etc/mcp-v8/sandbox.json
```

The sandbox is applied at startup, before the async runtime or V8 spawn a
single thread. Once applied it is irreversible for the life of the process,
and every thread and child process inherits it.

Grant paths must **exist when the server starts** (kernel rules attach to
real inodes; nono rejects missing paths), so create storage directories
before first launch. Directory grants are recursive; add
`"type": "file"` for single-file grants.

### What to grant for which feature

| You configure | Grant in the manifest |
|---|---|
| `--session-db-path` (always used) | that directory, `readwrite` |
| `--heap-store dir` / `--fs-store dir` | `--heap-dir` / `--fs-dir`, `readwrite` |
| `--cache-dir` (S3 cache) | that directory, `readwrite` |
| `--config`, `@file` prompts, `--wasm-module`, policy JSON files | each file, `read` |
| `file://` Rego policies | each policy file, `read` |
| `--allow-run-js-file` / `run_js_file` policy | the script roots, `read` |
| `--fs-passthrough` | the lower-layer roots, `read` |
| stdio MCP servers, subprocess policies | the tool binaries and whatever they touch |
| S3 backends | `~/.aws`, `read` (credentials/config) |

Everything in the system-paths block of the example (`/usr`, `/lib`, `/etc`,
`/proc/self`, `/dev/*`, plus `/nix` on Nix systems and `/System`, `/Library`,
`/private/etc` on macOS) is needed by essentially every deployment — dynamic
libraries, TLS roots, resolver config, entropy.

## Network

`network.mode` takes nono's values:

- `"unrestricted"` (nono's default) — no TCP restrictions.
- `"blocked"` — no network access. Add `ports` allowlists for listeners and
  specific egress:

```json
"network": {
  "mode": "blocked",
  "ports": { "bind": [8080], "connect": [443], "localhost": [8181] }
}
```

`bind` covers `--http-port`/`--sse-port`/`--cluster-port` listeners;
`connect` allows outbound TCP to a port number (any host); `localhost`
allows loopback-only traffic on a port (e.g. a local OPA server). Port-level
rules need Landlock ABI v4 (Linux 6.7+); on older kernels network filtering
is all-or-nothing and startup aborts if the manifest asks for a mix the
kernel cannot express.

Features that dial out — S3 stores, `--jwks-url`, clustering, JS `fetch()`,
remote OPA, SSE MCP servers, `--allow-external-modules` — need `unrestricted`
mode or the right `connect` ports (typically 443).

## Rejected manifest options

Parts of nono's manifest schema only work under nono's own CLI supervisor,
which runs your process as a child behind a filtering proxy. mcp-v8 applies
the sandbox **in-process**, where only kernel-expressible rules exist, so
these options are rejected at startup rather than silently ignored:

- `network.mode: "proxy"`, `network.allow_domains`, `network.endpoints`,
  `network.dns: false` — domain/L7/DNS filtering happens in nono's proxy.
  The kernel filters by **port**, not hostname.
- `credentials` — injection happens in nono's proxy.
- `rollback` — snapshots need a supervising parent process.
- `filesystem.deny` — Landlock is allow-list only; express denial by
  omitting grants.
- `process.exec_strategy: "supervised"`.

## Fail-closed contract

If the platform cannot enforce the manifest — an unsupported OS, a kernel
without Landlock — startup **aborts** with an explanatory error.
`--sandbox-manifest` never degrades to running unconfined.

```
Error: --sandbox-manifest requested but this system cannot enforce it (linux):
Landlock not available. Requires Linux kernel 5.13+ with Landlock enabled.
```

## Troubleshooting

Denials surface as `EACCES`/`Permission denied` errors from whatever tried
the access (a `run_js file=...` read, a subprocess, `fetch`). When something
legitimate is denied: find the path or port in the error, add the grant to
the manifest, restart — grants cannot be widened at runtime by design.

In a `--config` file the flag is a key like any other:

```toml
sandbox_manifest = "/etc/mcp-v8/sandbox.json"
```
