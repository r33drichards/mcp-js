# OS sandboxing

`--sandbox` adds a second, independent enforcement layer beneath the
[policy layer](policies.md), provided by the
[nono](https://github.com/nolabs-ai/nono) sandboxing library.

## Two layers, two failure domains

mcp-v8's security model is capability-based: `fetch`, filesystem, subprocess,
WASM, module imports, and upstream MCP calls are all off until an
[OPA/Rego policy](policies.md) grants them. That layer is expressive — it
decides per-request, with full context (URL, method, path, headers) — but it
runs *inside* the server process. Its guarantees assume the process itself
behaves: a V8 sandbox escape, a bug in a capability implementation, or a
misconfigured policy all live in the same failure domain.

The OS sandbox moves the backstop into the kernel:

| | Policy layer (OPA/Rego) | OS sandbox (`--sandbox`) |
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
the server builds a capability set from its own configuration — storage
directories read-write, configuration and policy files read-only, system
paths read-only, network per `--sandbox-network` — and applies it via
[nono](https://github.com/nolabs-ai/nono):

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
  set, the server refuses to start rather than run unconfined.

## What it does not do

The OS sandbox bounds *this process tree's* filesystem and TCP reach. It does
not rate-limit, inspect payloads, filter by hostname (kernel rules are
path- and port-shaped), or protect against a hostile host. For host-level
isolation run mcp-v8 in a container or VM; `--sandbox` then still narrows
what a compromised server can do inside it.

See [How-to: OS sandboxing](../how-to/os-sandbox.md) for flags, grants, and
troubleshooting.
