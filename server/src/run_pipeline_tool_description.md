Execute a pipeline of stages in a single call — no network round-trips between stages.

Each stage is either a **code** execution or a **heap** module load (never both). Output from one stage automatically pipes as `__stdin__` to the next. In stateful mode, heap snapshots chain between stages automatically.

params:
- stages: array of stage objects. Each stage has one of:
  - `{ "code": "..." }` — run JavaScript/TypeScript code (inherits heap from previous stage)
  - `{ "heap": "sha256hash" }` — load a pre-built heap snapshot as a reusable module
  - Optional per-stage overrides: `heap_memory_max_mb`, `execution_timeout_secs`
- session (optional): session name for logging all stages under one session

returns:
- stages: array of results, one per stage. Each has:
  - index: stage number (0-based)
  - output: evaluation result of the stage
  - heap: content hash of the stage's heap snapshot
  - stdout (if non-empty): console.log/info lines
  - stderr (if non-empty): console.error/warn lines


## Data flow

- **output → stdin**: The `output` of stage N becomes the `__stdin__` global of stage N+1
- **heap chaining**: The heap snapshot from stage N flows to stage N+1 automatically. A heap stage replaces the current heap with the specified snapshot.


## Heap as reusable module

Build utility functions once via `run_js`, then reference the heap hash in any pipeline:

```js
// Step 1: Build a utility module (via run_js)
// code: "function double(arr) { return arr.map(x => x * 2); }"
// → returns heap: "abc123..."

// Step 2: Use it in a pipeline
// stages: [
//   { "heap": "abc123..." },
//   { "code": "double([1, 2, 3, 4, 5])" }
// ]
// → Stage 0 loads the module, Stage 1 inherits it and calls double()
```


## Examples

**Code pipeline (piping output between stages):**
```json
{
  "stages": [
    { "code": "JSON.stringify([1, 2, 3, 4, 5])" },
    { "code": "JSON.parse(__stdin__).map(x => x * 2)" },
    { "code": "JSON.parse(__stdin__).reduce((a, b) => a + b, 0)" }
  ]
}
```
Result: `stages[2].output = "30"`

**Heap module + code:**
```json
{
  "stages": [
    { "heap": "abc123..." },
    { "code": "sum(double([1, 2, 3]))" }
  ]
}
```

**Multiple modules at different stages:**
```json
{
  "stages": [
    { "heap": "utils_hash" },
    { "code": "JSON.stringify(transform(data))" },
    { "heap": "validator_hash" },
    { "code": "validate(JSON.parse(__stdin__))" }
  ]
}
```
