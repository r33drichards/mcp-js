# Module Loading

External modules are disabled by default. When enabled with
`--allow-external-modules`, `mcp-v8` resolves npm and JSR specifiers through
`esm.sh` and also permits direct URL imports.

This is a networked module-loading model, not a local package-manager model.
Imports are resolved at runtime, and optional policy checks can audit them
before fetch.
