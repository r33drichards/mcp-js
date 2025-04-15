# mcp-v8 - MCP Project

A complete MCP project with both client and server components, using stdio transport.

## Features

- **Robust Communication**: Reliable stdio transport with proper error handling and timeout management
- **Multiple Connection Methods**: Connect to an already running server or start a new server process
- **Interactive Mode**: Choose tools and provide parameters interactively
- **One-shot Mode**: Run queries directly from the command line
- **Comprehensive Logging**: Detailed logging for debugging and monitoring

## Project Structure

- `client/`: The MCP client implementation
- `server/`: The MCP server implementation with tools
- `test.sh`: A test script to run both client and server

## Building

```bash
# Build the server
cd server
cargo build

# Build the client
cd ../client
cargo build
```

## Running

### Start the Server

```bash
cd server
cargo run
```

### Run the Client

```bash
cd client
cargo run -- --name "Your Name"
```

### Interactive Mode

```bash
cd client
cargo run -- --interactive
```

## Testing

```bash
./test.sh
```


```
cd server 
cargo build
npx @modelcontextprotocol/inspector target/debug/mcp-v8-server                                                              ~/mcp-v8/
```