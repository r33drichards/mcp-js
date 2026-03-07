#!/usr/bin/env -S deno run -A
//
// Test script for --secure-sessions mode.
//
// Spins up two MCP client connections with different session JWTs,
// uses Bedrock Claude Haiku to drive tool calls, and verifies that
// sessions are isolated from each other.
//
// Prerequisites:
//   - docker compose up -d
//   - AWS credentials configured (for Bedrock)
//   - JWT_SECRET env var matching docker-compose (default: "test-secret-change-me")
//
// Usage:
//   deno run -A scripts/test-secure-sessions.ts
//

import { Client } from "npm:@modelcontextprotocol/sdk@1.12.1/client/index.js";
import { StreamableHTTPClientTransport } from "npm:@modelcontextprotocol/sdk@1.12.1/client/streamableHttp.js";
import { SignJWT } from "npm:jose@6.0.11";
import {
  BedrockRuntimeClient,
  ConverseCommand,
} from "npm:@aws-sdk/client-bedrock-runtime@3.797.0";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const JWT_SECRET = Deno.env.get("JWT_SECRET") ?? "test-secret-change-me";
const MCP_URL = Deno.env.get("MCP_SERVER_URL") ?? "http://localhost:3000";
const BEDROCK_MODEL =
  Deno.env.get("BEDROCK_MODEL") ?? "us.anthropic.claude-haiku-4-5-20251001";
const BEDROCK_REGION = Deno.env.get("AWS_REGION") ?? "us-east-1";

const GLOBAL_UUID = crypto.randomUUID();
console.log(`Global test UUID: ${GLOBAL_UUID}`);
console.log(`MCP server:       ${MCP_URL}`);
console.log(`Bedrock model:    ${BEDROCK_MODEL}`);
console.log();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function mintJWT(sessionId: string): Promise<string> {
  const secret = new TextEncoder().encode(JWT_SECRET);
  return await new SignJWT({ session_id: sessionId })
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(secret);
}

async function createMcpClient(sessionId: string): Promise<Client> {
  const token = await mintJWT(sessionId);
  const transport = new StreamableHTTPClientTransport(
    new URL(`${MCP_URL}/mcp`),
    {
      requestInit: {
        headers: {
          "agent-session": token,
        },
      },
    },
  );
  const client = new Client({
    name: `test-client-${sessionId.slice(0, 8)}`,
    version: "1.0.0",
  });
  await client.connect(transport);
  return client;
}

/** Convert MCP tool schemas to Bedrock toolConfig format. */
function mcpToolsToBedrock(
  mcpTools: Awaited<ReturnType<Client["listTools"]>>,
) {
  return mcpTools.tools.map((t) => ({
    toolSpec: {
      name: t.name,
      description: t.description ?? "",
      inputSchema: {
        json: t.inputSchema,
      },
    },
  }));
}

const bedrock = new BedrockRuntimeClient({ region: BEDROCK_REGION });

/**
 * Run one agentic chat turn: send a user message, let Haiku call tools
 * against the MCP server, and return the final assistant text.
 */
async function chat(
  mcpClient: Client,
  tools: ReturnType<typeof mcpToolsToBedrock>,
  userMessage: string,
): Promise<string> {
  const messages: Array<Record<string, unknown>> = [
    { role: "user", content: [{ text: userMessage }] },
  ];

  // Tool-use loop (max 10 iterations as a safety net)
  for (let i = 0; i < 10; i++) {
    const resp = await bedrock.send(
      new ConverseCommand({
        modelId: BEDROCK_MODEL,
        messages: messages as never,
        toolConfig: { tools: tools as never },
      }),
    );

    const assistantContent = resp.output?.message?.content ?? [];
    messages.push({ role: "assistant", content: assistantContent });

    // Check if model wants to use tools
    const toolUses = assistantContent.filter(
      (b: Record<string, unknown>) => b.toolUse,
    );

    if (toolUses.length === 0) {
      // No more tool calls — extract final text
      const textBlocks = assistantContent.filter(
        (b: Record<string, unknown>) => b.text,
      );
      return textBlocks.map((b: Record<string, unknown>) => b.text).join("\n");
    }

    // Execute each tool call against MCP
    const toolResults = [];
    for (const block of toolUses) {
      const tu = (block as Record<string, Record<string, unknown>>).toolUse;
      console.log(`  -> tool call: ${tu.name}(${JSON.stringify(tu.input)})`);

      const mcpResult = await mcpClient.callTool({
        name: tu.name as string,
        arguments: tu.input as Record<string, unknown>,
      });

      const resultText = mcpResult.content
        .map((c: Record<string, unknown>) =>
          typeof c.text === "string" ? c.text : JSON.stringify(c)
        )
        .join("\n");

      console.log(`  <- result: ${resultText.slice(0, 200)}`);

      toolResults.push({
        toolResult: {
          toolUseId: tu.toolUseId,
          content: [{ text: resultText }],
        },
      });
    }

    messages.push({ role: "user", content: toolResults });
  }

  return "(max tool iterations reached)";
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

const sessionA = `${GLOBAL_UUID}-session-a`;
const sessionB = `${GLOBAL_UUID}-session-b`;

console.log("=== Creating MCP clients ===");
console.log(`Session A: ${sessionA}`);
console.log(`Session B: ${sessionB}`);
console.log();

const clientA = await createMcpClient(sessionA);
const clientB = await createMcpClient(sessionB);

const toolsA = await clientA.listTools();
const toolsB = await clientB.listTools();
console.log(
  `Session A tools: ${toolsA.tools.map((t) => t.name).join(", ")}`,
);
console.log(
  `Session B tools: ${toolsB.tools.map((t) => t.name).join(", ")}`,
);

// Verify 'session' param is NOT in the run_js tool schema (secure mode)
const runJsTool = toolsA.tools.find((t) => t.name === "run_js");
const runJsProps = (runJsTool?.inputSchema as Record<string, unknown>)
  ?.properties as Record<string, unknown> | undefined;
if (runJsProps && "session" in runJsProps) {
  console.error(
    "\nFAIL: run_js tool still exposes 'session' parameter — secure sessions not active!",
  );
  Deno.exit(1);
}
console.log(
  "OK: run_js tool does NOT expose 'session' parameter (secure mode confirmed)",
);
console.log();

const bedrockTools = mcpToolsToBedrock(toolsA);

// --- Session A: store a value ---
console.log("=== Session A: store a variable ===");
const responseA1 = await chat(
  clientA,
  bedrockTools,
  "Use run_js to execute this JavaScript code: `var secret = 42; secret;` — then get the execution result and tell me the value and the heap hash.",
);
console.log(`\nAssistant A: ${responseA1}\n`);

// --- Session A: read it back (with heap chaining) ---
console.log("=== Session A: read variable back ===");
const responseA2 = await chat(
  clientA,
  bedrockTools,
  "Now list the session snapshots to find the latest heap hash, then use run_js with that heap to execute: `secret + 8;` — tell me the result.",
);
console.log(`\nAssistant A: ${responseA2}\n`);

// --- Session B: try to access the variable (should be undefined) ---
console.log("=== Session B: attempt to access Session A's variable ===");
const responseB = await chat(
  clientB,
  bedrockTools,
  "Use run_js to execute: `typeof secret` — then get the result and tell me what it returned. This should be 'undefined' since this is a fresh session.",
);
console.log(`\nAssistant B: ${responseB}\n`);

// --- Cleanup ---
console.log("=== Closing connections ===");
await clientA.close();
await clientB.close();

console.log("\nDone. If Session A got 42 and 50, and Session B got 'undefined',");
console.log("then secure session isolation is working correctly.");
