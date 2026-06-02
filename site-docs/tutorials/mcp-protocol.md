# Tutorial: Integrating with AI Clients

In this tutorial you will connect mcp-v8 to four popular AI clients: Claude Desktop, Claude Code, Cursor, and Codex. Each client uses the MCP protocol to discover and call mcp-v8's tools automatically.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- At least one of the AI clients listed below installed

## Step 1: Understand how MCP integration works

When an AI client connects to mcp-v8 via the MCP protocol, it discovers the available tools (like `run_js`) and can call them as part of its workflow. The AI agent can then:

1. Write JavaScript/TypeScript code
2. Execute it via `run_js`
3. Inspect the results
4. Continue building on previous state

No special configuration is needed on the mcp-v8 side beyond choosing a transport.

## Step 2: Connect to Claude Desktop

Claude Desktop uses stdio transport to communicate with MCP servers.

### 2a: Edit the Claude Desktop configuration

Open the Claude Desktop configuration file:

- **macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Linux**: `~/.config/Claude/claude_desktop_config.json`

Add mcp-v8 to the `mcpServers` section:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": []
    }
  }
}
```

If you want to customize the server, add flags to `args`:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": [
        "--directory-path", "/tmp/mcp-v8-heaps",
        "--timeout", "60"
      ]
    }
  }
}
```

### 2b: Restart Claude Desktop

Close and reopen Claude Desktop. It will start the mcp-v8 process automatically.

### 2c: Verify the connection

In a new conversation, you should see mcp-v8's tools available. Ask Claude to "run some JavaScript" and it will use the `run_js` tool.

## Step 3: Connect to Claude Code

Claude Code also uses stdio transport for MCP servers.

### 3a: Add mcp-v8 to Claude Code

Run this command in your terminal:

```bash
claude mcp add mcp-v8 -- mcp-v8
```

To add with custom arguments:

```bash
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/mcp-v8-heaps
```

### 3b: Verify the connection

Start a Claude Code session and ask it to execute JavaScript. Claude Code will discover and use the `run_js` tool.

```bash
claude
```

Then type: "Use mcp-v8 to calculate the first 10 Fibonacci numbers"

Claude Code will write and execute the JavaScript, returning the results.

## Step 4: Connect to Cursor

Cursor supports MCP servers through its settings.

### 4a: Open Cursor settings

Go to **Cursor Settings** > **MCP** (or open `.cursor/mcp.json` in your project).

### 4b: Add mcp-v8

Add the following configuration:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": []
    }
  }
}
```

### 4c: Restart Cursor

Restart Cursor or reload the window. The mcp-v8 tools will be available in Cursor's AI chat.

### 4d: Test the integration

In Cursor's AI chat, ask: "Use the run_js tool to sort an array of numbers." Cursor will call `run_js` through the MCP protocol.

## Step 5: Connect to Codex

OpenAI's Codex CLI supports MCP servers.

### 5a: Configure Codex

Add mcp-v8 to your Codex configuration. Create or edit `.codex/config.json`:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": []
    }
  }
}
```

### 5b: Run Codex

Start a Codex session and it will discover the mcp-v8 tools.

## Step 6: Use SSE transport for remote connections

If the AI client is running on a different machine, or you need a network-accessible connection, use SSE transport:

```bash
mcp-v8 --sse-port 8080
```

Then configure the client to connect via SSE URL:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "url": "http://your-server:8080/sse"
    }
  }
}
```

This works with any MCP client that supports SSE transport.

## Step 7: Test with a practical example

Once connected to any client, try this workflow:

1. Ask the AI: "Create a JavaScript function that analyzes a list of numbers"
2. The AI will call `run_js` with the code
3. Ask: "Now extend that to include standard deviation"
4. The AI resumes from the previous heap and adds to the existing code
5. Ask: "Run it with some sample data"
6. The AI calls `run_js` again, building on the accumulated state

This iterative, stateful workflow is the core value proposition of mcp-v8 for AI agents.

## What you learned

- How to configure mcp-v8 as an MCP server in Claude Desktop, Claude Code, Cursor, and Codex
- That stdio transport is the standard for local AI client integration
- That SSE transport enables remote connections
- How AI agents use the `run_js` tool iteratively with stateful sessions

Next, learn about importing packages in [Importing External Packages](module-loading.md).
