# OS sandboxing (--sandbox-manifest)

`--sandbox-manifest <file.json>` confines the whole server process with an
OS-enforced sandbox — [Landlock](https://landlock.io/) on Linux (kernel
5.13+), Seatbelt on macOS — using a
[nono](https://github.com/nolabs-ai/nono) **capability manifest**. It is
defense in depth *underneath* the [OPA/Rego policy layer](policies.md): even
if V8, the policy chain, or the server itself were compromised, the kernel
still refuses filesystem and network access that was never granted.

nono capabilities compose additively, and the server uses that: the enforced
capability set is the union of

- **your manifest** — nono's own JSON schema and semantics, passed through
  verbatim — carrying the grants only you know about (script roots,
  passthrough directories, tool binaries) and the **network posture**, and
- **a server baseline** derived from the rest of the configuration —
  storage directories read-write, config/policy/WASM inputs read-only,
  system library paths read-only, and bind grants for configured listener
  ports.

You never restate what the server already knows about itself, and the
baseline never widens egress: outbound network access is exactly what your
manifest says.

See [Concepts: OS sandboxing](../concepts/os-sandbox.md) for the security
model.

## Turn it on

The smallest useful manifest sets only the network posture:

```json
{ "version": "0.1.0", "network": { "mode": "blocked" } }
```

```bash
mcp-v8 --heap-store dir --heap-dir /var/lib/mcp-v8/heaps \
  --sandbox-manifest /etc/mcp-v8/sandbox.json
```

The flag also accepts inline JSON, and in a [`--config` file](../reference/config-file.md)
the manifest is a structured `sandbox` section — no separate JSON file, no
quoting:

```toml
heap_store = "dir"
heap_dir = "/var/lib/mcp-v8/heaps"

[sandbox]
version = "0.1.0"
[sandbox.network]
mode = "blocked"
```

The NixOS module exposes the same section as a plain attrset, so the
manifest is ordinary Nix:

```nix
services.mcp-js.settings = {
  heap_store = "dir";
  heap_dir = "/var/lib/mcp-v8/heaps";
  sandbox = {
    version = "0.1.0";
    filesystem.grants = [
      { path = "/srv/scripts"; access = "read"; }
    ];
    network.mode = "blocked";
  };
};
```

At startup — before the async runtime or V8 spawn a single thread — the
server composes the manifest with the derived baseline and asks the kernel
to enforce it. The baseline covers:

- **Read-write**: the session db (`--session-db-path`), heap dir
  (`--heap-dir`), fs blob store (`--fs-dir`), fs label db
  (`--fs-labels-db`), and S3 cache (`--cache-dir`) — whichever are in play,
  created if missing.
- **Read-only**: the `--config` file, `@file` prompt overrides, WASM
  modules, the JSON files behind `--wasm-config` / `--mcp-config` /
  `--fetch-header-config` / `--policies-json`, local `file://` Rego
  policies, system paths (`/usr`, `/lib`, `/etc`, `/proc/self`, …), entropy
  devices, and `~/.aws` when an S3 backend is configured.
- **Listeners**: bind grants for `--http-port` / `--sse-port` /
  `--cluster-port` when the manifest blocks the network.

Once applied the sandbox is irreversible for the life of the process, and
every thread and child process inherits it.

## Grant extra paths

The baseline only sees paths named in the configuration. Anything else the
server should reach goes in the manifest — directory grants are recursive,
`"type": "file"` grants a single file, and grant paths must exist at
startup (kernel rules attach to real inodes; nono rejects missing paths):

```json
{
  "version": "0.1.0",
  "filesystem": {
    "grants": [
      { "path": "/srv/scripts", "access": "read" },
      { "path": "/opt/languages", "access": "read" },
      { "path": "/srv/output", "access": "readwrite" }
    ]
  },
  "network": { "mode": "blocked" }
}
```

Typical cases:

- `--allow-run-js-file` or a `run_js_file` policy: grant the script roots.
- `--fs-passthrough`: grant the read-only lower-layer roots.
- Subprocess policies that run tools outside the system paths: grant the
  tool and whatever it reads/writes.

## Control the network posture

Your manifest owns egress. `network.mode` takes nono's values:

- `"unrestricted"` (nono's default when the section is omitted) — no TCP
  restrictions.
- `"blocked"` — no network access, with optional `ports` allowlists:

```json
"network": {
  "mode": "blocked",
  "ports": { "connect": [443], "localhost": [8181] }
}
```

`connect` allows outbound TCP to a port number (any host); `localhost`
allows loopback-only traffic on a port (e.g. a local OPA server); `bind` is
available but rarely needed — configured listener ports are composed in
automatically. Port-level rules need Landlock ABI v4 (Linux 6.7+); on older
kernels network filtering is all-or-nothing and startup aborts if the
composed set asks for a mix the kernel cannot express.

Features that dial out — S3 stores, `--jwks-url`, clustering, JS `fetch()`,
remote OPA, SSE MCP servers, `--allow-external-modules` — need
`unrestricted` or the right `connect` ports (typically 443). If the
configuration enables such a feature while the manifest blocks the network,
the server warns at startup and the feature fails at runtime; the manifest
wins.

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

If the platform cannot enforce the composed capability set — an unsupported
OS, a kernel without Landlock — startup **aborts** with an explanatory
error. `--sandbox-manifest` never degrades to running unconfined.

```
Error: --sandbox-manifest requested but this system cannot enforce it (linux):
Landlock not available. Requires Linux kernel 5.13+ with Landlock enabled.
```

## Troubleshooting

Denials surface as `EACCES`/`Permission denied` errors from whatever tried
the access (a `run_js file=...` read, a subprocess, `fetch`). When something
legitimate is denied: find the path or port in the error, add the grant to
the manifest, restart — grants cannot be widened at runtime by design.

`RUST_LOG=info` logs the composition at startup (`OS sandbox applied
(linux): manifest ... composed with server baseline ...`).

In a `--config` file, use either the structured `sandbox` section shown
above or a `sandbox_manifest` path key (they are mutually exclusive):

```toml
sandbox_manifest = "/etc/mcp-v8/sandbox.json"
```
