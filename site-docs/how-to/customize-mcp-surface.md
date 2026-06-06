# Customize the prompt and tool descriptions

Two flags let an operator override the text mcp-v8 presents to MCP clients,
without rebuilding the server:

- `--instructions` — overrides the server **`instructions`** (the "system prompt"
  returned during `initialize`).
- `--run-js-description` — overrides the description advertised for the **`run_js`**
  tool in `tools/list`.

Both apply in stateful and stateless mode. The `run_js` override is selective —
all other built-in tools keep their default descriptions.

## Value syntax: inline text or `@file`

Each flag's value is used **verbatim as inline text**, unless it begins with `@`:

| Value | Meaning |
|-------|---------|
| `"some text"` | Used literally as the override |
| `@./prompt.txt` | Read the override from the file `./prompt.txt` |
| `@@text` | Literal leading `@` — produces `@text` |

## Override the server instructions

```bash
# Inline
mcp-v8 --http-port 8080 --instructions "You can run JavaScript for me via run_js."

# From a file
mcp-v8 --http-port 8080 --instructions @./prompts/instructions.md
```

The text is returned to the client in the `initialize` response, replacing the
default instructions.

## Override the run_js tool description

```bash
# Inline
mcp-v8 --http-port 8080 --run-js-description "Execute JavaScript/TypeScript in a sandbox."

# From a file (handy for long, multi-paragraph guidance)
mcp-v8 --http-port 8080 --run-js-description @./prompts/run_js.md
```

The text replaces the `run_js` entry's description in `tools/list`; other tools
are unaffected.

## See also

- [Transports](../concepts/transports.md) — what the MCP surface exposes per transport.
- [Calling upstream MCP servers](../how-to/mcp-client.md) — composing other MCP servers.
- [CLI flags reference](../reference/cli-flags.md) — all server flags.
