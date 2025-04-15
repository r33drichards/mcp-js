import { McpServer, ResourceTemplate } from "npm:@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "npm:@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "npm:zod";

import { DefaultApi, Configuration } from "./typescript-axios-client-generated";

// Create an MCP server
const server = new McpServer({
  name: "Demo",
  version: "1.0.0"
});

// Add an addition tool
server.tool("add",
  { a: z.number(), b: z.number() },
  async ({ a, b }) => ({
    content: [{ type: "text", text: String(a + b) }]
  })
);

server.tool("javascript",
  { code: z.string() },
  async ({ code }) => {
    console.error("javascript", code);
    const api = new DefaultApi(
      new Configuration({
        basePath: "http://localhost:8000"
      })

    );
    try {
      const result = await api.handlersJavascriptJavascript({ code: code });
      console.error("result", result.data);
      return { content: [{ type: "text", text: String(result.data.result) }] };
    } catch (error) {
      return { content: [{ type: "text", text: String(error) }] };
    }
  }
);

// Add a dynamic greeting resource
server.resource(
  "greeting",
  new ResourceTemplate("greeting://{name}", { list: undefined }),
  async (uri, { name }) => ({
    contents: [{
      uri: uri.href,
      text: `Hello, ${name}!`
    }]
  })
);

// Start receiving messages on stdin and sending messages on stdout
const transport = new StdioServerTransport();
console.log("Starting server...");
await server.connect(transport);