/**
 * RunJS MCP Server - Edge Case Test Results
 *
 * These tests were run against the RunJS MCP tool to document its behavior
 * at various edge cases and boundaries. Each test documents the input,
 * expected behavior, and actual result.
 *
 * Environment: Deno-based V8 isolate (stateless shell mode)
 * Temporal API: Supported
 * TypeScript: Type stripping only (no type checking)
 */

// ============================================================
// 1. CONSOLE OUTPUT
// ============================================================

// Test 1: All console methods work with prefixes
// console.log("msg")    -> "msg"
// console.info("msg")   -> "[INFO] msg"
// console.warn("msg")   -> "[WARN] msg"
// console.error("msg")  -> "[ERROR] msg"

// Test 2: No implicit return of last expression
// `const x = 42; x;` -> empty output (must use console.log)

// Test 3: No output for silent expressions
// `const silent = 1 + 1;` -> empty output

// Test 4: console.log with multiple args
// `console.log("a", "b", 1, 2)` -> "a b 1 2"
// Objects get JSON-stringified: `{key:"value"}` -> `{"key":"value"}`
// undefined becomes empty string in multi-arg output

// ============================================================
// 2. TYPESCRIPT SUPPORT
// ============================================================

// Test 5: Valid TS interfaces and typed functions work correctly
// Types are stripped before execution, code runs as JS

// Test 6: INVALID types are silently stripped (not reported as errors)
// `const val: CompletelyMadeUpType = "hello"` -> works fine, outputs "hello"

// Test 7: TS enums generate runtime code and work correctly
// `enum Color { Red, Green, Blue }` -> Color.Red === 0

// Test 8: Generic functions work
// `function identity<T>(arg: T): T` -> types stripped, function works

// ============================================================
// 3. ERROR HANDLING
// ============================================================

// Test 9: Uncaught throw -> returns in `error` field with stack trace
// `throw new Error("boom")` -> error: "Error: boom\n    at <eval>:2:7"

// Test 10: Syntax errors -> TypeScript parse error (parsed as TS first)
// Invalid syntax -> "TypeScript parse error: Error { error: ... }"

// Test 11: ReferenceError -> standard V8 error
// `undefinedVariable` -> "ReferenceError: undefinedVariable is not defined"

// Test 12: TypeError -> standard V8 error
// `null.property` -> "TypeError: Cannot read properties of null"

// Test 13: PARTIAL OUTPUT SURVIVES ERRORS
// console.log before throw -> output contains pre-error logs AND error field
// This is important: output and error can both be populated

// ============================================================
// 4. ASYNC / AWAIT
// ============================================================

// Test 14-17: TOP-LEVEL AWAIT DOES NOT WORK
// Despite docs claiming support, `await` at top level gives:
// "SyntaxError: await is only valid in async functions and the top level bodies of modules"

// Test 19-21: ASYNC IIFE WORKAROUND WORKS
// `(async () => { const r = await Promise.resolve("ok"); console.log(r); })();`
// The runtime waits for the IIFE's promise to resolve before returning

// ============================================================
// 5. RESOURCE LIMITS
// ============================================================

// Test 22: Timeout works - infinite loop with 2s timeout
// `while(true){}` with execution_timeout_secs=2 -> error: "Execution timed out"

// Test 23: Memory within bounds works
// 1M element array with 100-char strings at 8MB heap -> success

// Test 24: OOM is caught gracefully
// Exceeding heap_memory_max_mb=4 -> error: "Out of memory: V8 heap limit exceeded"

// Test 25: Stack overflow is caught
// Infinite recursion -> "RangeError: Maximum call stack size exceeded"

// ============================================================
// 6. UNAVAILABLE APIs
// ============================================================

// Test 26: setTimeout -> "setTimeout is not defined"
// Test 27: setInterval -> "setInterval is not defined"
// Test 28: fetch -> "fetch is not defined" (no --opa-url configured)
// Test 29: DOM APIs:
//   - document -> not defined
//   - window -> not defined
//   - navigator -> undefined (exists on globalThis but is undefined)
//   - localStorage -> not defined

// Test 30: Runtime environment:
//   - require("fs") -> "require is not defined" (no CommonJS)
//   - process -> not defined (no Node.js)
//   - Deno -> EXISTS (runtime is Deno-based!)
//   - Bun -> not defined

// Test 31: Available globals (71 total):
// Standard JS builtins + Deno, Temporal, console, queueMicrotask
// Notable: Deno.version returns empty, Deno only exposes `core` key

// ============================================================
// 7. EXTERNAL MODULES
// ============================================================

// Test 33: Static npm imports are DISABLED
// `import { camelCase } from "npm:lodash-es@4.17.21"`
// -> "External module imports are disabled. Start server with --allow-external-modules"

// Test 34: Dynamic imports also fail (top-level await not supported)

// ============================================================
// 8. OUTPUT EDGE CASES
// ============================================================

// Test 36: Unicode and special characters work correctly
// Chinese, emoji, Greek, math symbols all render properly
// Null char (\0), zero-width space, tabs, newlines all pass through

// Test 37: Large output (1000 lines x 80+ chars = ~90KB) succeeds
// No truncation by the runtime itself (client may truncate display)

// Test 38-39: Empty code / just comments -> empty output, no error

// ============================================================
// 9. ISOLATION
// ============================================================

// Test 40-41: Each execution is a FRESH V8 isolate
// Prototype pollution in one execution does NOT persist to the next
// `Object.prototype.polluted = "yes"` in call A -> not visible in call B

// ============================================================
// 10. MODERN JS FEATURES
// ============================================================

// Test 42: WeakRef and FinalizationRegistry work
// WeakRef.deref() returns the object (GC hasn't run in short-lived isolate)

// Test 43: Temporal API is SUPPORTED
// `Temporal.Now.plainDateISO()` returns current date correctly

// ============================================================
// SUMMARY OF KEY FINDINGS
// ============================================================
//
// 1. Runtime: Deno-based V8 isolate (stateless, fresh per call)
// 2. TypeScript: Type-stripping only (invalid types silently ignored)
// 3. Top-level await: BROKEN (despite docs) - use async IIFE instead
// 4. External imports: DISABLED on this server instance
// 5. Partial output: Preserved even when execution throws
// 6. Temporal API: Fully supported (cutting-edge JS feature)
// 7. TS Enums: Work correctly (generate runtime code)
// 8. Limits: Timeout, OOM, and stack overflow all caught gracefully
// 9. Isolation: Perfect - no state leaks between executions
// 10. No fetch/setTimeout/setInterval/DOM/fs/process available
// 11. console.log is the ONLY way to produce output (no implicit returns)
// 12. `undefined` values in multi-arg console.log render as empty strings
