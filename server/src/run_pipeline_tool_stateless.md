Execute a pipeline of code stages in a single call — no network round-trips between stages.

Each stage runs JavaScript/TypeScript code. Output from one stage automatically pipes as `__stdin__` to the next. Each stage gets a fresh V8 isolate.

params:
- stages: array of stage objects. Each stage has:
  - `{ "code": "..." }` — JavaScript/TypeScript code to run
  - Optional per-stage overrides: `heap_memory_max_mb`, `execution_timeout_secs`

returns:
- stages: array of results, one per stage. Each has:
  - index: stage number (0-based)
  - output: evaluation result of the stage
  - stdout (if non-empty): console.log/info lines
  - stderr (if non-empty): console.error/warn lines


## Data flow

- **output → stdin**: The `output` of stage N becomes the `__stdin__` global of stage N+1


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

**Transform pipeline:**
```json
{
  "stages": [
    { "code": "JSON.stringify({ name: 'Alice', score: 42 })" },
    { "code": "var d = JSON.parse(__stdin__); d.score *= 2; JSON.stringify(d)" },
    { "code": "JSON.parse(__stdin__).score" }
  ]
}
```
Result: `stages[2].output = "84"`
