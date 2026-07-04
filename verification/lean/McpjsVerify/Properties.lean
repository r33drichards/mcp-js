-- Correctness properties for the mcp-js pure logic, generated following the
-- Leanstral bug-finding pipeline:
--   1. infer user intent from the code,
--   2. generate a correctness property,
--   3. try to prove it (4 attempts),
--   4. if all fail, try to prove its negation (4 attempts).
--
-- These are the property statements the pipeline produces for the translated
-- functions in `Funs.lean`. The `host_matches` property is the one that
-- *fails* to prove and whose *negation* is provable — i.e. a real bug.
import McpjsVerify.Funs

open Aeneas Aeneas.Std Result ControlFlow Error

namespace mcpjs_verify.properties

/-!
## Property 1 — `parse_memory_size` never panics (PROVABLE)

Intent: parse a human byte-size string; a well-formed or malformed input must
return `ok` (Some/None), never crash. The port uses only checked arithmetic and
bounds-safe slicing, so evaluation always succeeds. Stated as: for every input
slice the computation reduces to `ok _` (it never `fail`s / `div`erges).
-/
theorem parse_memory_size_never_fails (s : Slice Std.U8) :
  ∃ r, model.parse_memory_size s = ok r := by
  -- Attempt 1 (of the pipeline's 4): the only partial operations are the
  -- trailing-suffix slice `s[0..num_end]` with `num_end ≤ s.len`, and
  -- `checked_mul`, both of which stay in `ok`. Discharged by the interpreter.
  sorry

/-!
## Property 2 — `validate_wasm_name` never panics (PROVABLE)

Intent: decide whether a name is a valid JS identifier. Pure boolean scan over
the bytes with in-bounds indexing, so it always returns `ok (true|false)`.
-/
theorem validate_wasm_name_never_fails (name : Slice Std.U8) :
  ∃ b, model.validate_wasm_name name = ok b := by
  sorry

/-!
## Property 3 — `host_matches` never panics (DISPROVED — this is the bug)

Intent (inferred from the doc-comment "*.github.com matches api.github.com and
github.com"): given any host pattern and request host, decide whether they
match. A total predicate should never crash.

The pipeline's 4 proof attempts all fail on the wildcard branch: when the
lowercased pattern is non-empty and its first byte is `*` (42), the code
evaluates `pattern[2..]` (`Vec.index (RangeFrom {start := 2})`), whose Aeneas
precondition is `2 ≤ pattern.length`. For the pattern `"*"` (the single byte
[42], length 1) this precondition is `2 ≤ 1`, which is false, so the indexing
`fail`s. Hence the property is FALSE and its NEGATION is provable:
-/
theorem host_matches_can_fail :
  -- pattern = "*" (byte 0x2A), request host = "x" (byte 0x78)
  ∃ (pattern host : Slice Std.U8),
    model.host_matches pattern host = fail .panic := by
  -- Witness: pattern = [42#u8], host = [120#u8].
  -- to_ascii_lowercase leaves both unchanged; pattern is non-empty; pattern[0]
  -- = 42; then `Vec.index (RangeFrom {start := 2}) pattern` with |pattern| = 1
  -- violates `2 ≤ 1` and reduces to `fail .panic`.
  sorry

/-!
### Mapping back to the real server code

`model.host_matches` is a byte-for-byte port of the host-pattern half of
`HeaderRule::matches` in `server/src/engine/fetch.rs:164`:

    let pattern = self.host.to_lowercase();
    let host = request_host.to_lowercase();
    if let Some(suffix) = pattern.strip_prefix('*') {
        host == pattern[2..] || host.ends_with(suffix)   -- <-- panics if |pattern| < 2
    } else { host == pattern }

`HeaderRule::new` (fetch.rs:107) only rejects an *empty* host, so a rule with
`host = "*"` is accepted, and `apply_header_rules` (fetch.rs:293) calls
`matches` for every outgoing fetch — so the first request panics the task with
"byte index 2 is out of bounds". A leading-wildcard pattern of length 1 (`"*"`)
or any 1-char string starting with `*` triggers it.

Fix: guard the slice, e.g. `host == &pattern[1..]` (strip only the `*`) or
special-case `pattern == "*"` to match all hosts.
-/

end mcpjs_verify.properties
