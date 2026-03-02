# RunJS MCP Server — Security Findings

**Date**: 2026-03-02
**Tested against**: Current `main` branch (post-neutralization fixes from commits 599278b, 9d48df0)

---

## CRITICAL

### 1. `op_print` neutralization bypass via prototype chain → raw stdout write

**Severity**: Critical
**Impact**: JSON-RPC protocol stream injection; potential MCP client/model manipulation

The `Deno.core.print` neutralization uses `Object.create(origCore)` to shadow the
`print` property. However, the original core object remains accessible via the
prototype chain:

```js
const origCore = Object.getPrototypeOf(Deno.core);
origCore.print("arbitrary bytes to stdout\n", false);  // bypasses neutralization
origCore.print("arbitrary bytes to stderr\n", true);    // also bypasses
```

**Root cause**: deno_core's bootstrap defines `Deno.core.print = (msg, isErr) => op_print(msg, isErr)` where `op_print` is the *original native Rust function* captured in a closure at bootstrap time. The neutralization replaces `Deno.core.ops.op_print` but that doesn't affect the closure reference. The shadow property on `newCore` overrides `Deno.core.print`, but `Object.getPrototypeOf(Deno.core).print` still reaches the original.

**Confirmed**: Output from `origCore.print(...)` does NOT appear in the captured
console output — it goes directly to the process's stdout/stderr file descriptors.
On stdio transport, this means it enters the JSON-RPC stream.

**Recommended fix**: After creating `newCore`, also neutralize the prototype:

```js
Object.defineProperty(origCore, 'print', {
    value: safePrint,
    writable: false,
    configurable: false,
});
```

Or alternatively, replace the `Deno.core` property with `configurable: false` and
also freeze the prototype path:

```js
Object.setPrototypeOf(newCore, null);  // sever prototype chain entirely
// copy needed properties explicitly instead of inheriting
```

---

### 2. `Deno.core.ops` is mutable — all ops replaceable/interceptable

**Severity**: Critical (stateful mode) / Medium (stateless mode)
**Impact**: Persistent op interception, data exfiltration, trust boundary violation

`Deno.core.ops` is not frozen, sealed, or made non-extensible:

```js
Object.isExtensible(Deno.core.ops)  // true
Object.isFrozen(Deno.core.ops)      // false
Object.isSealed(Deno.core.ops)      // false
```

All ops have `writable: true, configurable: true`. Any op can be replaced with
a wrapper function:

```js
const orig = Deno.core.ops.op_console_write;
Deno.core.ops.op_console_write = function(msg, level) {
    exfiltrate(msg);           // steal all console output
    return orig.call(this, msg, level);
};
```

In **stateful mode**, this persists across V8 heap snapshot save/restore cycles,
meaning a single malicious execution installs a permanent trojan intercepting all
subsequent executions in that session.

**Recommended fix**: After neutralization, freeze the ops object:

```js
Object.freeze(Deno.core.ops);
```

---

## HIGH

### 3. `Deno.core` property is `configurable: true` — neutralization reversible

**Severity**: High
**Impact**: Entire print/panic neutralization can be undone

```js
Object.defineProperty(globalThis.Deno, 'core', {
    value: Object.getPrototypeOf(Deno.core),  // restore original core
    writable: false, configurable: true
});
// Deno.core.print is now the original native print again
```

**Recommended fix**: Set `configurable: false` on the `Deno.core` property.

---

### 4. Event loop hooks persist in stateful mode

**Severity**: High (stateful) / Low (stateless)
**Impact**: Persistent code execution across sessions

All event-loop callback registration APIs succeed and persist:

```js
__bootstrap.core.setMacrotaskCallback(() => { /* runs every macrotask */ });
__bootstrap.core.setNextTickCallback(() => { /* runs on next tick */ });
__bootstrap.core.setImmediateCallback(() => { /* runs on immediate */ });
__bootstrap.core.setPromiseHooks(init, before, after, resolve);
__bootstrap.core.setHandledPromiseRejectionHandler((p, r) => { /* intercept */ });
__bootstrap.core.setReportExceptionCallback((e) => { /* suppress errors */ });
__bootstrap.core.setWasmStreamingCallback((src, rid) => { /* intercept WASM */ });
__bootstrap.core.addMainModuleHandler((spec) => { /* hijack imports */ });
```

In stateful mode, these persist across snapshot boundaries.

**Recommended fix**: After running user code (before snapshotting), reset all
callbacks. Or freeze `__bootstrap.core` to prevent registration.

---

### 5. Prototype pollution persists in stateful mode

**Severity**: High (stateful) / Low (stateless)
**Impact**: Cross-execution data corruption and trust violation

All built-in prototypes are mutable:

```js
Object.prototype.__polluted__ = "yes";
Array.prototype.map = function() { /* intercepted */ };
JSON.stringify = function() { /* intercepted */ };
Promise = class Evil extends Promise { /* intercepted */ };
```

In stateful mode, these modifications persist across executions, allowing a
malicious first execution to poison all subsequent ones.

**Recommended fix**: In stateful mode, consider freezing critical prototypes
before user code runs, or verify prototype integrity after execution.

---

### 6. `op_get_proxy_details` bypasses Proxy handlers

**Severity**: High
**Impact**: Any Proxy-based access control or encapsulation can be bypassed

```js
const secret = { password: "hunter2" };
const proxy = new Proxy(secret, {
    get(target, prop) { return prop === "password" ? "***" : target[prop]; }
});
proxy.password                                 // "***" (handler applied)
Deno.core.ops.op_get_proxy_details(proxy)      // [{password:"hunter2"}, {}] (bypassed!)
```

This breaks any library or framework that uses Proxies for encapsulation.

**Recommended fix**: Remove or neutralize `op_get_proxy_details`.

---

## MEDIUM

### 7. `__bootstrap` global exposes powerful internal APIs

**Severity**: Medium
**Impact**: Access to runtime internals not intended for user code

`globalThis.__bootstrap` exposes:
- `__bootstrap.core` — full internal core API (100+ methods)
- `__bootstrap.primordials` — pristine built-in constructors (including `Function`)
- `__bootstrap.internals` — internal registration object

The `primordials.Function` constructor can create arbitrary code even if `Function`
were removed from the global scope.

**Recommended fix**: Delete `globalThis.__bootstrap` after setup, or
`Object.defineProperty(globalThis, '__bootstrap', { value: undefined })`.

---

### 8. SharedArrayBuffer available (Spectre timer prerequisite)

**Severity**: Medium
**Impact**: High-resolution timing primitive for side-channel attacks

```js
const sab = new SharedArrayBuffer(1024);
const view = new Int32Array(sab);
Atomics.store(view, 0, 42);  // works
```

While Workers are not available (limiting the classic Spectre timer), SAB
availability is a prerequisite. If Workers were ever added, this becomes a
Spectre vector.

**Recommended fix**: Consider disabling `SharedArrayBuffer` if not needed.

---

### 9. ReDoS can burn full execution timeout

**Severity**: Medium
**Impact**: CPU exhaustion for the duration of the execution timeout (default 30s)

```js
/^(a+)+$/.test("a".repeat(28) + "b");  // hangs for 30s until timeout
```

The execution timeout catches this, but 30 seconds of CPU burn per invocation
is still a resource concern under high concurrency.

**Recommended fix**: Consider V8's `--regexp-backtrace-limit` flag, or a lower
default timeout for regex-heavy code patterns.

---

### 10. `globalThis` and `Deno` are extensible — can be frozen to break future sessions

**Severity**: Medium (stateful) / Low (stateless)
**Impact**: Denial of service in stateful mode

```js
Object.freeze(globalThis);     // prevents any future property additions
Object.freeze(Deno);           // breaks Deno.core access patterns
```

In stateful mode, this would break all subsequent executions in the session.

**Recommended fix**: Run user code, then verify global object integrity before
snapshotting. Or use a fresh isolate per execution even in stateful mode.

---

## LOW

### 11. Information leakage via `op_memory_usage`

Exposes V8 heap statistics: `{physicalTotal, heapTotal, heapUsed, external}`.

### 12. `import.meta` exposes filesystem paths

`import.meta.filename` → `/main_N.js`, `import.meta.dirname` → `/`,
`import.meta.url` → `file:///main_N.js`. Reveals execution environment details.

### 13. `op_destructure_error` leaks detailed stack frames

Returns full file paths, line/column numbers, and stack trace details.

### 14. `op_is_terminal` probes stdio file descriptors

Returns whether fds 0-4 are terminals (all false in container), confirming
execution environment characteristics.

### 15. `eval()` and `Function` constructor available

Both `eval()` and `new Function()` work, enabling dynamic code generation.
This is standard for V8 but means content-based code filtering can be trivially
bypassed via string obfuscation.

---

## Summary of what IS working well

- **op_panic neutralization**: Solid — no closure bypass available. The replaced
  JS function correctly throws instead of calling Rust `panic!()`.
- **Module import restrictions**: All non-http(s) schemes properly rejected
  (`data:`, `blob:`, `file:`, `javascript:`, `node:`, `ext:`).
- **Heap memory limits**: OOM properly caught and returned as error.
- **Execution timeout**: Works correctly for CPU exhaustion scenarios.
- **Resource table isolation**: Empty resource table; `op_write`/`op_read` to
  stdout/stderr FDs fail with "Bad resource ID".
- **No Node.js/Deno APIs**: `process`, `require`, `Buffer`, `Deno.env` all absent.
- **op_import_sync restricted**: Only allows http/https modules.
- **Console capture**: Works correctly for `console.log` and friends.
