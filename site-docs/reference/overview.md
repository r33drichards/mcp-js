# Reference

Complete, factual reference material: flags, configuration keys, tool parameters, and
endpoints. For learning and tasks, see the [Tutorials](../tutorials/overview.md)
and [How-to guides](../how-to/overview.md).

Generated reference pages are rebuilt from their source of truth and must not be
edited by hand:

- [CLI flags](cli-flags.md) — every command-line flag (from the Clap definition).
- [Configuration file](config-file.md) — every `--config` file key (from the Clap definition and the config loader's tables).
- [HTTP API](http-api.md) — the REST surface (from `openapi.json` via Widdershins).
- [MCP tools](mcp-tools.md) — the built-in MCP tools (from the tool registry).

Hand-written platform reference:

- [Native UniFFI bindings](uniffi-bindings.md) — artifacts, exported API groups, and platform constraints.

For feature-level explanations and behaviour, see the
[Concepts](../concepts/overview.md) section; for task recipes, see the
[How-to guides](../how-to/overview.md).
