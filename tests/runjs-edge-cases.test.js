/**
 * RunJS MCP Server - Comprehensive Edge Case & Security Test Results
 *
 * These tests were run against the RunJS MCP tool to document its behavior
 * at various edge cases, boundaries, and adversarial inputs.
 *
 * Environment: Deno-based V8 isolate (stateless shell mode)
 * Temporal API: Supported
 * TypeScript: Type stripping only (no type checking)
 */

// ============================================================
// 1. CONSOLE OUTPUT
// ============================================================

// Test: All console methods work with prefixes
// console.log("msg")    -> "msg"           (level 0)
// console.info("msg")   -> "[INFO] msg"    (level 1)
// console.warn("msg")   -> "[WARN] msg"    (level 2)
// console.error("msg")  -> "[ERROR] msg"   (level 3)
// console.debug("msg")  -> "msg"           (level 0, same as log)
// console.trace("msg")  -> "msg"           (level 0, same as log)

// Test: No implicit return of last expression
// `const x = 42; x;` -> empty output (must use console.log)

// Test: console.log with multiple args
// `console.log("a", "b", 1, 2)` -> "a b 1 2"
// Objects get JSON-stringified: `{key:"value"}` -> `{"key":"value"}`
// undefined becomes empty string in multi-arg output

// Test: Console internals revealed
// console.log is NOT native - it's a JS wrapper:
//   function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 0); }
// console.warn uses level 2, error uses level 3, info uses level 1
// console is { configurable: true, writable: true, enumerable: false }

// ============================================================
// 2. TYPESCRIPT SUPPORT
// ============================================================

// Test: Valid TS interfaces and typed functions work correctly
// Types are stripped before execution, code runs as JS

// Test: INVALID types are silently stripped (not reported as errors)
// `const val: CompletelyMadeUpType = "hello"` -> works fine, outputs "hello"

// Test: TS enums generate runtime code and work correctly
// `enum Color { Red, Green, Blue }` -> Color.Red === 0

// Test: Generic functions work
// `function identity<T>(arg: T): T` -> types stripped, function works

// ============================================================
// 3. ERROR HANDLING
// ============================================================

// Test: Uncaught throw -> returns in `error` field with stack trace
// `throw new Error("boom")` -> error: "Error: boom\n    at <eval>:2:7"

// Test: Syntax errors -> TypeScript parse error (parsed as TS first)
// Invalid syntax -> "TypeScript parse error: Error { error: ... }"

// Test: PARTIAL OUTPUT SURVIVES ERRORS
// console.log before throw -> output contains pre-error logs AND error field
// Both `output` and `error` fields can be populated simultaneously

// ============================================================
// 4. ASYNC / AWAIT
// ============================================================

// Test: TOP-LEVEL AWAIT DOES NOT WORK
// Despite docs claiming support, `await` at top level gives:
// "SyntaxError: await is only valid in async functions and the top level bodies of modules"

// Test: ASYNC IIFE WORKAROUND WORKS
// `(async () => { const r = await Promise.resolve("ok"); console.log(r); })();`
// The runtime waits for the IIFE's promise to resolve before returning

// Test: Deeply nested microtask chains via async IIFE all resolve correctly

// ============================================================
// 5. RESOURCE LIMITS
// ============================================================

// Test: Timeout works - infinite loop with 2s timeout
// `while(true){}` with execution_timeout_secs=2 -> error: "Execution timed out"

// Test: OOM is caught gracefully
// Exceeding heap_memory_max_mb=4 -> error: "Out of memory: V8 heap limit exceeded"

// Test: Stack overflow is caught
// Infinite recursion -> "RangeError: Maximum call stack size exceeded"

// Test: ReDoS (catastrophic backtracking) NOT mitigated
// `/^(a+)+$/` on "a".repeat(25)+"!" takes 2238ms
// No regex execution timeout - potential DoS vector for CPU exhaustion

// ============================================================
// 6. NODE.JS COMPATIBILITY
// ============================================================

// Test: ZERO Node.js compatibility
// require("fs/path/os/...") -> "require is not defined"
// Buffer: undefined
// process: undefined
// __dirname: undefined
// __filename: undefined
// module: undefined
// exports: undefined
// global: undefined
// structuredClone: not defined
// TextEncoder: not defined (use Deno.core.encode instead)
// Worker: not defined
// performance: not defined

// ============================================================
// 7. EXECUTION MODE
// ============================================================

// Test: Code runs in SLOPPY MODE (not strict)
// `with` statement works: eval('with({x:42}) { ... }') -> x is 42
// `arguments` is typeof "undefined" (available but empty at top level)
// __proto__ is writable: obj.__proto__ = { injected: true } works
// constructor.constructor trick works: [].constructor.constructor("return typeof Deno")() -> "object"

// ============================================================
// 8. UNAVAILABLE APIs
// ============================================================

// setTimeout/setInterval: not defined
// fetch: not defined (no --opa-url configured)
// DOM: document, window, localStorage not defined
// Deno high-level APIs: ALL stripped
//   readTextFile, readFile, env, cwd, execPath, hostname, uid,
//   Command, open, stat, readDir, writeTextFile, remove, makeTempDir,
//   listen, connect -> all "not a function"
// Deno.pid: undefined (exists but returns nothing)

// ============================================================
// 9. EXTERNAL MODULES
// ============================================================

// Static npm/jsr/URL imports: DISABLED
// -> "External module imports are disabled. Start server with --allow-external-modules"

// op_import_sync("node:fs"): "Module loading is not supported"
// op_lazy_load_esm("ext:*"): "cannot be lazy-loaded"
// import.meta: "Cannot use 'import.meta' outside a module"

// ============================================================
// 10. ISOLATION
// ============================================================

// Test: Each execution is a FRESH V8 isolate
// Prototype pollution in one execution does NOT persist to the next

// Test: structuredClone not available but Deno.core.serialize/deserialize works
// V8 serialization with crafted payloads -> RangeError for invalid data (no crash)

// ============================================================
// 11. MODERN JS FEATURES
// ============================================================

// Temporal API: SUPPORTED (Temporal.Now.plainDateISO() works)
// WeakRef/FinalizationRegistry: work
// SharedArrayBuffer + Atomics: WORK (but no Workers for timing attacks)
// WebAssembly: FULLY WORKS - can compile and run wasm modules
// Proxy/Reflect: work (can proxy globalThis)
// Symbol.toPrimitive: works

// ====================================================================
// ====================================================================
//
//  SECURITY FINDINGS - DENO.CORE INTERNALS EXPOSED
//
// ====================================================================
// ====================================================================

// ============================================================
// 12. [CRITICAL] op_panic - SERVER CRASH (DoS)
// ============================================================
//
// Deno.core.ops.op_panic("message")
//
// Calls Rust's panic!() macro. CRASHES THE ENTIRE MCP SERVER PROCESS.
// The server session expires and must be restarted.
// Any user code can trivially DoS the server with a single call.
//
// Severity: CRITICAL
// Impact: Complete denial of service
// Mitigation: Remove op_panic from the ops object before executing user code

// ============================================================
// 13. [HIGH] op_dispatch_exception - Isolate Termination
// ============================================================
//
// Deno.core.ops.op_dispatch_exception(new Error("msg"), false, false)
//
// Immediately terminates the isolate with the given error.
// Bypasses try/catch - cannot be caught by user code.
// Partial output before the call IS preserved.
//
// setReportExceptionCallback does NOT intercept it.

// ============================================================
// 14. [HIGH] Deno.core.print - Output Stream Bypass
// ============================================================
//
// Deno.core.print("text\n")
//
// Writes DIRECTLY to the process's stdout/stderr, bypassing the
// console capture mechanism. Output from Deno.core.print does NOT
// appear in the `output` field of the response - it goes to raw stdout.
//
// If the MCP server uses stdio transport, this could potentially
// corrupt the JSON-RPC protocol stream with injected messages.
// Tested: Deno.core.print('{"jsonrpc":"2.0",...}\n') - gets swallowed
// (output not in response), so the server may be using a pipe/fd redirect.

// ============================================================
// 15. [HIGH] op_console_write - Direct Console API
// ============================================================
//
// Deno.core.ops.op_console_write("message\n", level)
//
// Direct access to the output capture mechanism. Levels:
//   0 = log (no prefix)
//   1 = [INFO]
//   2 = [WARN]
//   3 = [ERROR]
//   -1, 4, 5 = no prefix
//
// Can inject fake log level prefixes: op_console_write("[ERROR] fake", 0)
// Can be MONKEY-PATCHED to intercept/modify all console output:
//   Deno.core.ops.op_console_write = function(msg, level) {
//     orig("INJECTED | " + msg, level);
//   };
//   -> All subsequent console.log calls go through the patched version

// ============================================================
// 16. [MEDIUM] Event Loop Manipulation
// ============================================================
//
// setMacrotaskCallback: can register callbacks into the event loop
// setNextTickCallback: can register Node.js-style next-tick callbacks
// setImmediateCallback: can register immediate callbacks
// runMicrotasks(): manually drain the microtask queue
// runImmediateCallbacks(): manually trigger immediate callbacks
// eventLoopTick(): manually advance the event loop
// eventLoopHasMoreWork(): query event loop state
//
// Successfully re-implemented callback scheduling:
//   Deno.core.setImmediateCallback(() => { ... });
//   Deno.core.runImmediateCallbacks(); // -> callback fires!
//   Deno.core.setMacrotaskCallback(() => { ... });
//   Deno.core.eventLoopTick(); // -> callback fires!

// ============================================================
// 17. [MEDIUM] eval() and Function Constructor
// ============================================================
//
// eval() works and accesses enclosing scope
// new Function("return ...") works and sees Deno global
// Deno.core.evalContext(code, specifier) works with URL specifiers
// evalContext can set properties on globalThis:
//   evalContext(`globalThis.__escaped = true`, "data:text/javascript,")
//   -> globalThis.__escaped === true

// ============================================================
// 18. [MEDIUM] Monkey-Patching Attacks
// ============================================================
//
// ALL built-in prototypes and globals are mutable within an execution:
// - console.log can be replaced
// - JSON.stringify can be poisoned
// - Error constructor can be wrapped
// - Object.keys can filter properties
// - Array.prototype.forEach can be intercepted
// - Symbol.iterator can be overridden
// - Object.prototype.toJSON poisons all JSON.stringify calls
// - op_console_write itself can be monkey-patched
//
// Impact: Within a single execution, all output can be manipulated.
// Mitigated by: Fresh isolate per execution (no cross-execution persistence)

// ============================================================
// 19. [LOW] Information Leaks
// ============================================================
//
// Deno.core.memoryUsage() -> { physicalTotal, heapTotal, heapUsed, external }
// Deno.core.build -> { target: "unknown", arch: "unknown", os: "unknown" } (scrubbed)
// Date.now() available for timing (but coarse resolution, all 0 deltas)
// performance.now() NOT available (good)
// CPU speed fingerprinting via busy loops possible (10M iters = ~4ms)
// 101 registered ops visible via Object.keys(Deno.core.ops)
// 71 globals visible via Object.getOwnPropertyNames(globalThis)

// ============================================================
// 20. [LOW] __bootstrap Internals
// ============================================================
//
// __bootstrap.primordials: 75 frozen copies of built-in methods
//   - Includes uncurryThis, FunctionPrototypeCall, Proxy, Function
//   - Can call eval through primordials: FunctionPrototypeCall(eval, null, "1+1")
// __bootstrap.internals: empty object, but WRITABLE
//   - Can set arbitrary properties (e.g. internals.permissions = {...})
//   - Unknown if anything reads from it later
// __bootstrap.core: same as Deno.core

// ============================================================
// 21. [INFO] WebAssembly Works
// ============================================================
//
// Can compile and instantiate arbitrary WebAssembly modules.
// Tested: add(a,b) function compiled from hand-crafted wasm bytes.
// This means arbitrary native-speed computation is possible,
// though still sandboxed within V8's wasm sandbox.

// ============================================================
// 22. [INFO] Deno.core.ops - Full Op List (101 ops)
// ============================================================
//
// Dangerous ops:
//   op_panic(msg)                    - CRASHES SERVER (Rust panic!)
//   op_dispatch_exception(err,b,b)   - Terminates isolate
//   op_print(msg, isStderr)          - Raw stdout/stderr write
//   op_close(fd)                     - Close file descriptors
//   op_read_sync(fd, buf)            - Read from fd (BadResource - no open fds)
//   op_write_sync(fd, buf)           - Write to fd (BadResource - no open fds)
//
// Event loop ops:
//   op_timer_queue, op_timer_cancel, op_timer_ref, op_timer_unref
//   (Could not get timer ops to accept arguments - type mismatch)
//
// Serialization ops:
//   op_serialize, op_deserialize     - V8 serialization format
//   (Malformed payloads -> RangeError, no crashes)
//
// Utility ops:
//   op_encode, op_decode             - UTF-8 encode/decode
//   op_eval_context                  - Evaluate code with URL specifier
//   op_memory_usage                  - Heap statistics
//   op_resources                     - List open resources (returns [])
//   op_import_sync                   - "Module loading is not supported"
//   op_lazy_load_esm                 - "cannot be lazy-loaded"
//   op_console_write                 - Direct console output

// ====================================================================
// SUMMARY OF KEY FINDINGS
// ====================================================================
//
// CRITICAL VULNERABILITIES:
//   1. op_panic -> instant server crash (DoS)
//
// HIGH SEVERITY:
//   2. Deno.core.print bypasses output capture (potential protocol injection)
//   3. op_dispatch_exception kills isolate bypassing try/catch
//   4. op_console_write is monkey-patchable (output manipulation)
//
// MEDIUM SEVERITY:
//   5. Full event loop control via core APIs
//   6. eval/Function/evalContext all work (dynamic code execution)
//   7. Sloppy mode enables with statement and other legacy features
//   8. ReDoS not mitigated (no regex timeout)
//
// DESIGN OBSERVATIONS:
//   9.  Runtime is Deno-based with only Deno.core exposed
//   10. All high-level Deno APIs properly stripped
//   11. File descriptors properly closed (BadResource on all fd ops)
//   12. Fresh V8 isolate per execution (no state leaks)
//   13. External imports correctly disabled
//   14. V8 deserialization handles malformed input gracefully
//   15. WebAssembly fully functional (sandboxed)
//   16. Temporal API supported
//   17. Zero Node.js compatibility (no require, Buffer, process, etc.)
//   18. Build info properly scrubbed ("unknown" everywhere)
//
// RECOMMENDED FIXES:
//   - Remove op_panic from the exposed ops (or wrap it to no-op)
//   - Freeze Deno.core.ops to prevent monkey-patching
//   - Remove or restrict Deno.core.print to prevent output bypass
//   - Consider restricting evalContext
//   - Add regex execution timeouts
//   - Consider running in strict mode
