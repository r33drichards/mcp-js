# Node Globals And Modal Tutorial Design

Date: 2026-08-19

## Summary

Add an opt-in `--node-globals` compatibility flag that installs the existing
`node:buffer` `Buffer` export and synthetic `node:process` default export on
`globalThis` before user modules are linked or evaluated. Keep the default
runtime unchanged and web-compatible.

Update the Modal tutorial to enable the flag and import the bundled Node build
from esm.sh. The bundled build avoids Modal's optional native-detection path,
which currently imports the unsupported `node:child_process` builtin.

## Goals

- Add `--node-globals` CLI support.
- Add matching `MCP_V8_NODE_GLOBALS` environment-variable support.
- Add matching `node_globals` config-file support.
- Install `globalThis.Buffer` and `globalThis.process` before user module
  evaluation when enabled.
- Preserve the current default global surface when the option is disabled.
- Update the Modal tutorial so its example works with current `modal` packages.
- Cover CLI, config, runtime ordering, and default-off behavior with tests.

## Non-Goals

- Claim full Node.js compatibility.
- Add other Node globals such as `global`, `__filename`, or `__dirname`.
- Add an importable `node:child_process` module.
- Expand subprocess, filesystem, networking, CommonJS, or package-resolution
  compatibility.
- Change the existing `node:buffer` or synthetic `node:process`
  implementations.
- Enable the option by default.

## Configuration

The option follows the existing precedence model:

1. CLI flag
2. Environment variable
3. Config file
4. Default (`false`)

Supported forms:

```text
--node-globals
MCP_V8_NODE_GLOBALS=true
node_globals = true
```

The option is independent of `--allow-external-modules`. It can be used for
inline code and built-in modules without allowing network-fetched packages.

## Runtime Behavior

When disabled, `globalThis.Buffer` and `globalThis.process` remain absent unless
user code assigns them.

When enabled, runtime setup must evaluate an internal bootstrap module before
the user entry module is instantiated:

```javascript
import { Buffer } from 'node:buffer';
import process from 'node:process';

globalThis.Buffer = Buffer;
globalThis.process = process;
```

The bootstrap must reuse the embedded compatibility modules so global and
imported values have identical behavior. In particular, `process.env` remains
empty and no host process state is exposed.

Installing the globals before module evaluation is the defining behavior. It
allows a dependency to read `process` or `Buffer` during its top-level module
initialization, which cannot be achieved by assignments placed after a static
import in user source.

## Modal Tutorial

The server command gains `--node-globals`.

The JavaScript example removes manual `Buffer` and `process` imports and uses:

```javascript
import { ModalClient } from 'npm:modal?target=node&bundle';
```

The `target=node` query selects the Node SDK build. The `bundle` query prevents
the external dependency graph from loading the optional `detect-libc` path that
imports `node:child_process`.

The tutorial should explain that:

- `--node-globals` provides `Buffer` and the sandboxed `process` shim before
  dependency evaluation.
- The flag does not expose host environment variables or grant filesystem,
  subprocess, or network capabilities.
- Modal still requires the existing HTTP/2 policy, external-module permission,
  disabled heap snapshots, and host-scoped credential injection.

Container configuration gains:

```text
MCP_V8_NODE_GLOBALS=true
```

The Railway example should include the same variable.

## Testing

Tests should prove:

- CLI parsing accepts `--node-globals`.
- `MCP_V8_NODE_GLOBALS` and `node_globals` participate in normal precedence.
- The default runtime does not define `Buffer` or `process`.
- The enabled runtime exposes the same values as `node:buffer` and
  `node:process`.
- A statically imported test module can read both globals during top-level
  evaluation, proving bootstrap ordering.
- Existing Node compatibility tests continue to pass.

The Modal cloud call itself remains tutorial validation rather than a required
CI test because it needs external credentials and billable infrastructure.

## Documentation

Update generated CLI/config references through the repository's existing
documentation workflow if those files are generated from option metadata.
Document the option as a narrow compatibility convenience, not a Node.js mode.

## Security

The flag adds no new host capability. It exposes only existing in-isolate
compatibility values:

- `Buffer` is the embedded JavaScript polyfill.
- `process` uses fixed sandbox values and an empty environment.

The option remains explicit because the presence of `process` can cause npm
packages to select Node-specific code paths. Users should enable it only when
those paths are intended.
