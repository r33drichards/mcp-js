# Architecture of MCP-v8

MCP-v8 is a JavaScript server that runs within a WebAssembly environment, enabling secure and efficient execution of JavaScript code in a controlled sandbox.

## Core Components

### 1. WebAssembly Runtime
The core of MCP-v8 is the WebAssembly runtime that executes JavaScript code. This provides:
- Sandboxed execution environment
- Memory safety guarantees
- Performance optimization through compilation to WebAssembly

### 2. MCP Protocol Handler
MCP (Model Control Protocol) is a protocol for communication between AI models and tools. The handler:
- Processes incoming MCP requests
- Manages session state
- Translates between MCP format and JavaScript execution

### 3. Transport Layer
MCP-v8 supports multiple transport mechanisms:
- HTTP/REST API
- STDIO (command-line interface)
- Server-Sent Events (SSE)

## Execution Model

The execution model follows these steps:
1. Request arrives via one of the supported transports
2. MCP protocol handler parses the request
3. JavaScript code is executed in the WebAssembly sandbox
4. Results are formatted according to MCP specification
5. Response is sent back through the same transport

## Security Considerations

MCP-v8 implements several security measures:
- Sandboxed execution environment
- Limited access to system resources
- Configurable security policies
- Memory usage limits
