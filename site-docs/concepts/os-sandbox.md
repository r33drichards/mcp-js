# OS sandboxing

`--sandbox-manifest` adds a second, independent enforcement layer beneath
the [policy layer](policies.md), provided by the
[nono](https://github.com/nolabs-ai/nono) sandboxing library and configured
by a nono capability manifest — nono's own JSON format, passed through
verbatim.

## Two layers, two failure domains

mcp-v8's security model is capability-based: `fetch`, filesystem, subprocess,
WASM, module imports, and upstream MCP calls are all off until an
[OPA/Rego policy](policies.md) grants them. That layer is expressive — it
decides per-request, with full context (URL, method, path, headers) — but it
runs *inside* the server process. Its guarantees assume the process itself
behaves: a V8 sandbox escape, a bug in a capability implementation, or a
misconfigured policy all live in the same failure domain.

The OS sandbox moves the backstop into the kernel:

| | Policy layer (OPA/Rego) | OS sandbox (`--sandbox-manifest`) |
|---|---|---|
| Enforced by | the server process | the kernel (Landlock / Seatbelt) |
| Granularity | per request, full context | per path / per port |
| Configurable | per capability, hot-swappable chains | fixed at startup, irreversible |
| Protects against | JS asking for too much | the *process* doing too much |

The two compose: a `fetch` call must pass the Rego chain *and* the kernel's
network posture; a `run_js file=...` read must pass the `run_js_file` policy
*and* land inside a granted path. Neither layer replaces the other — the
policy layer is the fine-grained front door, the OS sandbox is what's left
standing if the front door is bypassed.

## How enforcement works

At startup, before the async runtime or the V8 platform spawn any threads,
the server loads the [nono capability manifest](../how-to/os-sandbox.md)
named by `--sandbox-manifest` — filesystem grants, network mode, port
allowlists, all in nono's own schema and semantics, converted with nono's
manifest-to-capability-set conversion — and **composes** it with a baseline
derived from the server's own configuration (storage directories
read-write; config, policy, and system paths read-only; bind grants for
configured listeners). nono capabilities are an additive allow-list, so
composition is a union: the manifest can only widen the baseline, never
narrow it, and the baseline never widens the manifest's *network egress*
posture. The composed set is then applied:

- **Linux**: [Landlock](https://landlock.io/) LSM rules (kernel 5.13+;
  per-port TCP rules need ABI v4, kernel 6.7+, with a seccomp fallback for
  all-or-nothing network blocking on older kernels).
- **macOS**: a Seatbelt profile.

Three properties follow:

- **Irreversible** — there is no API to widen a live sandbox. New grants
  require a restart.
- **Inherited** — every thread and every child process (stdio MCP servers,
  policy-gated subprocesses) is confined by the same rules.
- **Fail-closed** — if the kernel cannot enforce the requested capability
  set, the server refuses to start rather than run unconfined. Manifest
  options that only work under nono's CLI supervisor (proxy-based domain
  filtering, credential injection, rollback, deny rules) are likewise
  rejected at startup instead of being silently ignored — this server
  applies the sandbox in-process, so only kernel-expressible rules exist.

The split keeps each layer owning what it knows: the server contributes the
grants that follow mechanically from its configuration, the manifest
contributes deployment-specific grants (script roots, tool binaries) and
the egress decision. A minimal `{"version": "0.1.0"}` manifest is valid and
yields a confined server with baseline grants only.

## What it does not do

The OS sandbox bounds *this process tree's* filesystem and TCP reach. It does
not rate-limit, inspect payloads, filter by hostname (kernel rules are
path- and port-shaped), or protect against a hostile host. For host-level
isolation run mcp-v8 in a container or VM; `--sandbox-manifest` then still
narrows what a compromised server can do inside it.

See [How-to: OS sandboxing](../how-to/os-sandbox.md) for the manifest
format, grants, and troubleshooting.
