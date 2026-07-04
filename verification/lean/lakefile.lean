import Lake
open Lake DSL

-- The translated code and its properties depend on the Aeneas Lean library
-- (which itself pulls in mathlib). Point `require aeneas` at a local checkout
-- of the Aeneas repo's `backends/lean` directory, or at the git dependency:
--
--   require aeneas from "PATH_TO_AENEAS_REPO/backends/lean"
--
-- or:
--
--   require aeneas from git
--     "https://github.com/AeneasVerif/aeneas" @ "main" / "backends" / "lean"

require aeneas from git
  "https://github.com/AeneasVerif/aeneas" @ "main" / "backends" / "lean"

package «mcpjs_verify» where

@[default_target] lean_lib «McpjsVerify» where
