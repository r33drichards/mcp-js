# How to Use the mcp-v8-cli Command-Line Client

Interact with a running mcp-v8 server from the terminal.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | sudo bash
```

## Set the server URL

By default, mcp-v8-cli connects to `http://localhost:3000`. Override with:

```bash
mcp-v8-cli --url http://your-server:3000 exec 'console.log(1)'
```

Or set the environment variable:

```bash
export MCP_V8_URL=http://your-server:3000
```

## Submit code for execution

```bash
mcp-v8-cli exec 'console.log(1 + 1);'
```

With a heap to resume from:

```bash
mcp-v8-cli exec --heap abc123... 'counter += 1; console.log(counter);'
```

With a session name:

```bash
mcp-v8-cli exec --session my-analysis 'var x = 10;'
```

With tags:

```bash
mcp-v8-cli exec --tag env=prod --tag project=demo 'console.log("tagged");'
```

With execution limits:

```bash
mcp-v8-cli exec --heap-memory-max-mb 16 --execution-timeout-secs 60 'console.log("big job");'
```

## List executions

```bash
mcp-v8-cli executions list
```

## Get execution status

```bash
mcp-v8-cli executions get <execution-id>
```

## Read console output

```bash
mcp-v8-cli executions output <execution-id>
```

With pagination:

```bash
mcp-v8-cli executions output <execution-id> --line-offset 0 --line-limit 50
mcp-v8-cli executions output <execution-id> --byte-offset 0 --byte-limit 4096
```

## Cancel an execution

```bash
mcp-v8-cli executions cancel <execution-id>
```

## Raw JSON output

Add `--json` or `-j` for machine-readable output:

```bash
mcp-v8-cli -j exec 'console.log(42);'
```
