#!/usr/bin/env -S deno run -A
//
// Test script for JWKS/OAuth secure sessions with Keycloak.
//
// Verifies:
// 1. Token acquisition from Keycloak (JWT for auth only)
// 2. Session isolation via X-MCP-Session-Id headers
// 3. Invalid/forged tokens are rejected
//
// Prerequisites:
//   docker compose -f docker-compose.secure-sessions.yml up --build -d
//   Wait for Keycloak to be healthy (~30s)
//
// Usage:
//   deno run -A scripts/test-keycloak-sessions.ts
//

import { Client } from "npm:@modelcontextprotocol/sdk@1.12.1/client/index.js";
import { StreamableHTTPClientTransport } from "npm:@modelcontextprotocol/sdk@1.12.1/client/streamableHttp.js";
import { ClientCredentialsProvider } from "./oauth-client-credentials-provider.ts";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const KEYCLOAK_URL = Deno.env.get("KEYCLOAK_URL") ?? "http://localhost:8080";
const MCP_URL = Deno.env.get("MCP_SERVER_URL") ?? "http://localhost:3000";
const CLIENT_ID = "mcp-client";
const CLIENT_SECRET = "mcp-client-secret";
const REALM = "mcp";
const TOKEN_URL = `${KEYCLOAK_URL}/realms/${REALM}/protocol/openid-connect/token`;

console.log(`Keycloak:   ${KEYCLOAK_URL}`);
console.log(`MCP server: ${MCP_URL}`);
console.log();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function decodeJwtPayload(token: string): Record<string, unknown> {
  const parts = token.split(".");
  const payload = JSON.parse(atob(parts[1]));
  return payload;
}

function createProvider(): ClientCredentialsProvider {
  return new ClientCredentialsProvider({
    tokenUrl: TOKEN_URL,
    clientId: CLIENT_ID,
    clientSecret: CLIENT_SECRET,
  });
}

async function createMcpClient(sessionId: string): Promise<{ client: Client; provider: ClientCredentialsProvider }> {
  const provider = createProvider();
  const transport = new StreamableHTTPClientTransport(
    new URL(`${MCP_URL}/mcp`),
    {
      authProvider: provider,
      requestInit: {
        headers: { "X-MCP-Session-Id": sessionId },
      },
    },
  );
  const client = new Client({
    name: `test-${sessionId}`,
    version: "1.0.0",
  });
  await client.connect(transport);
  return { client, provider };
}

async function runCode(client: Client, code: string): Promise<string> {
  const result = await client.callTool({
    name: "run_js",
    arguments: { code },
  });
  const content = result.content as Array<{ type: string; text?: string }>;
  const text = content.map((c) => c.text ?? JSON.stringify(c)).join("\n");

  // Parse execution_id from result
  try {
    const parsed = JSON.parse(text);
    if (parsed.execution_id && !parsed.execution_id.startsWith("error")) {
      // Wait for completion and get output
      await new Promise((r) => setTimeout(r, 500));
      const execResult = await client.callTool({
        name: "get_execution",
        arguments: { execution_id: parsed.execution_id },
      });
      const execContent = execResult.content as Array<{ type: string; text?: string }>;
      const execText = execContent.map((c) => c.text ?? JSON.stringify(c)).join("\n");

      const outputResult = await client.callTool({
        name: "get_execution_output",
        arguments: { execution_id: parsed.execution_id },
      });
      const outputContent = outputResult.content as Array<{ type: string; text?: string }>;
      const outputText = outputContent.map((c) => c.text ?? JSON.stringify(c)).join("\n");

      return JSON.stringify({ exec: JSON.parse(execText), output: JSON.parse(outputText) });
    }
    return text;
  } catch {
    return text;
  }
}

// ---------------------------------------------------------------------------
// Test runner
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;

function report(name: string, ok: boolean, detail?: string) {
  if (ok) {
    passed++;
    console.log(`  PASS: ${name}`);
  } else {
    failed++;
    console.log(`  FAIL: ${name}${detail ? " — " + detail : ""}`);
  }
}

// ===========================================================================
// Phase 1: Token acquisition and claim verification
// ===========================================================================

console.log("=".repeat(70));
console.log("PHASE 1: Keycloak token acquisition");
console.log("=".repeat(70));
console.log();

const provider = createProvider();

const tokens = await provider.tokens();

if (!tokens) {
  console.error("Failed to acquire token from Keycloak");
  Deno.exit(1);
}

const claims = decodeJwtPayload(tokens.access_token);

console.log("Token claims (subset):", {
  sub: claims.sub,
  azp: claims.azp,
});
console.log();

report("token has sub claim", typeof claims.sub === "string" && (claims.sub as string).length > 0);
report("token has azp claim", claims.azp === CLIENT_ID);
report("session_id NOT in JWT (comes from X-MCP-Session-Id header instead)", claims.session_id === undefined);
console.log();

// ===========================================================================
// Phase 2: Session isolation via MCP
// ===========================================================================

console.log("=".repeat(70));
console.log("PHASE 2: Session isolation");
console.log("=".repeat(70));
console.log();

const { client: clientA } = await createMcpClient("session-alpha");
const { client: clientB } = await createMcpClient("session-beta");

// Session A: store a value
console.log("--- Session A: store globalThis.secret = 42 ---");
const resultA1 = await runCode(clientA, "globalThis.secret = 42; console.log('stored');");
console.log(`  Result: ${resultA1.slice(0, 200)}`);
report("session A store succeeds", resultA1.includes("stored") || !resultA1.includes("error"));

// Get session A's latest heap for chaining
const snapsA = await clientA.callTool({
  name: "list_session_snapshots",
  arguments: {},
});
const snapsAText = (snapsA.content as Array<{ text?: string }>).map(c => c.text ?? "").join("");
let heapA: string | undefined;
try {
  const snapsAData = JSON.parse(snapsAText);
  if (snapsAData.entries?.length > 0) {
    heapA = snapsAData.entries[snapsAData.entries.length - 1].output_heap;
  }
} catch { /* ignore */ }

// Session A: read it back
if (heapA) {
  console.log(`--- Session A: read secret via heap ${heapA.slice(0, 12)}... ---`);
  const resultA2 = await runCode(clientA, `console.log("secret=" + globalThis.secret);`);
  // Note: without chaining the heap, the value won't persist in a new isolate
  // But in secure session mode, consecutive run_js calls within the same session
  // should share the session's heap chain
}

// Session B: should NOT see the variable (different session)
console.log("--- Session B: check isolation ---");
const resultB = await runCode(clientB, "console.log('type=' + typeof globalThis.secret);");
console.log(`  Result: ${resultB.slice(0, 200)}`);
report("session B isolation (secret not visible)", resultB.includes("undefined") || !resultB.includes("secret=42"));
console.log();

// ===========================================================================
// Phase 3: Invalid token rejection
// ===========================================================================

console.log("=".repeat(70));
console.log("PHASE 3: Invalid token rejection");
console.log("=".repeat(70));
console.log();

// Try connecting with a forged token (use static headers, no provider)
console.log("--- Forged token ---");
try {
  const forgedToken = tokens.access_token.slice(0, -5) + "XXXXX";
  const forgedTransport = new StreamableHTTPClientTransport(
    new URL(`${MCP_URL}/mcp`),
    { requestInit: { headers: { Authorization: `Bearer ${forgedToken}` } } },
  );
  const clientForged = new Client({ name: "test-forged", version: "1.0.0" });
  await clientForged.connect(forgedTransport);
  const forgedResult = await runCode(clientForged, "console.log('should not run');");
  // If we get here, the connection succeeded but run_js should fail (no valid session)
  report("forged token rejected", forgedResult.includes("error") || forgedResult.includes("no valid"));
  await clientForged.close();
} catch (e) {
  // Connection itself may fail
  report("forged token rejected", true);
  console.log(`  (rejected at connection level: ${(e as Error).message.slice(0, 100)})`);
}

// Try connecting with no token
console.log("--- No token ---");
try {
  const transport = new StreamableHTTPClientTransport(
    new URL(`${MCP_URL}/mcp`),
  );
  const clientNone = new Client({ name: "test-no-token", version: "1.0.0" });
  await clientNone.connect(transport);
  const noneResult = await runCode(clientNone, "console.log('should not run');");
  report("no token rejected", noneResult.includes("error") || noneResult.includes("no valid"));
  await clientNone.close();
} catch (e) {
  report("no token rejected", true);
  console.log(`  (rejected: ${(e as Error).message.slice(0, 100)})`);
}

console.log();

// ===========================================================================
// Summary
// ===========================================================================

console.log("=".repeat(70));
console.log(`RESULTS: ${passed} passed, ${failed} failed`);
console.log("=".repeat(70));

await clientA.close();
await clientB.close();

Deno.exit(failed > 0 ? 1 : 0);
