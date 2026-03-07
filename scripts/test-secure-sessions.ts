#!/usr/bin/env -S deno run -A
//
// Test script for --secure-sessions mode and OPA filesystem policy.
//
// 1. Session isolation: two MCP connections with different JWTs verify
//    that globalThis state doesn't leak between sessions.
// 2. Filesystem sandbox: agents write to /data/workspace/<uuid>/ and then
//    try every creative trick they can think of to escape the sandbox.
//
// Prerequisites:
//   - docker compose -f docker-compose.secure-sessions.yml up -d
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
  Deno.env.get("BEDROCK_MODEL") ??
    "us.anthropic.claude-haiku-4-5-20251001-v1:0";
const BEDROCK_REGION = Deno.env.get("AWS_REGION") ?? "us-east-1";

const GLOBAL_UUID = crypto.randomUUID();
const WORKSPACE = `/data/workspace/${GLOBAL_UUID}`;
console.log(`Global test UUID: ${GLOBAL_UUID}`);
console.log(`Workspace:        ${WORKSPACE}`);
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
  return mcpTools.tools.map((t: { name: string; description?: string; inputSchema: unknown }) => ({
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

  for (let i = 0; i < 15; i++) {
    const resp = await bedrock.send(
      new ConverseCommand({
        modelId: BEDROCK_MODEL,
        messages: messages as never,
        toolConfig: { tools: tools as never },
      }),
    );

    const assistantContent = resp.output?.message?.content ?? [];
    messages.push({ role: "assistant", content: assistantContent });

    const toolUses = assistantContent.filter(
      (b: Record<string, unknown>) => b.toolUse,
    );

    if (toolUses.length === 0) {
      const textBlocks = assistantContent.filter(
        (b: Record<string, unknown>) => b.text,
      );
      return textBlocks
        .map((b: Record<string, unknown>) => b.text)
        .join("\n");
    }

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

      console.log(`  <- result: ${resultText.slice(0, 300)}`);

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
// Phase 1: Session isolation
// ===========================================================================

console.log("=".repeat(70));
console.log("PHASE 1: Secure session isolation");
console.log("=".repeat(70));
console.log();

const sessionA = `${GLOBAL_UUID}-session-a`;
const sessionB = `${GLOBAL_UUID}-session-b`;

console.log(`Session A: ${sessionA}`);
console.log(`Session B: ${sessionB}`);
console.log();

const clientA = await createMcpClient(sessionA);
const clientB = await createMcpClient(sessionB);

const toolsA = await clientA.listTools();
const bedrockTools = mcpToolsToBedrock(toolsA);

// Check that 'session' param is hidden in secure mode
const runJsTool = toolsA.tools.find((t: { name: string }) => t.name === "run_js");
const runJsProps = (runJsTool?.inputSchema as Record<string, unknown>)
  ?.properties as Record<string, unknown> | undefined;
report(
  "run_js hides 'session' parameter",
  !runJsProps || !("session" in runJsProps),
);
console.log();

// Session A: store a value
console.log("--- Session A: store globalThis.secret = 42 ---");
const responseA1 = await chat(
  clientA,
  bedrockTools,
  "Use run_js to execute: `globalThis.secret = 42;` then get_execution to confirm it completed. Tell me the heap hash.",
);
console.log(`\n  Assistant: ${responseA1}\n`);

// Session A: read it back via heap chain
console.log("--- Session A: read secret + 8 via heap chain ---");
const responseA2 = await chat(
  clientA,
  bedrockTools,
  "List session snapshots to find the latest output_heap, then run_js with that heap: `console.log(globalThis.secret + 8);` and get the output. Tell me the console output.",
);
console.log(`\n  Assistant: ${responseA2}\n`);
report("heap chain returns 50", /50/.test(responseA2));

// Session B: should NOT see the variable
console.log("--- Session B: check isolation ---");
const responseB = await chat(
  clientB,
  bedrockTools,
  "Use run_js to execute: `console.log(typeof globalThis.secret);` then get the output. Tell me what it printed.",
);
console.log(`\n  Assistant: ${responseB}\n`);
report("session isolation (undefined)", /undefined/.test(responseB));

// ===========================================================================
// Phase 2: Filesystem sandbox — happy path
// ===========================================================================

console.log();
console.log("=".repeat(70));
console.log("PHASE 2: Filesystem sandbox — allowed operations");
console.log("=".repeat(70));
console.log();

console.log("--- Write and read a file inside workspace ---");
const fsHappy = await chat(
  clientA,
  bedrockTools,
  `Use run_js to execute the following JavaScript. Report exactly what console.log prints.
\`\`\`js
await fs.mkdir("${WORKSPACE}", { recursive: true });
await fs.writeFile("${WORKSPACE}/hello.txt", "sandbox works!");
const content = await fs.readFile("${WORKSPACE}/hello.txt");
console.log(content);
\`\`\``,
);
console.log(`\n  Assistant: ${fsHappy}\n`);
report("write + read inside workspace", /sandbox works/.test(fsHappy));

console.log("--- List directory ---");
const fsList = await chat(
  clientA,
  bedrockTools,
  `Use run_js to execute and report the console output:
\`\`\`js
const entries = await fs.readdir("${WORKSPACE}");
console.log(JSON.stringify(entries));
\`\`\``,
);
console.log(`\n  Assistant: ${fsList}\n`);
report("readdir inside workspace", /hello\.txt/.test(fsList));

// ===========================================================================
// Phase 3: Filesystem sandbox — breakout attempts
// ===========================================================================

console.log();
console.log("=".repeat(70));
console.log("PHASE 3: Filesystem sandbox — breakout attempts (agent-driven)");
console.log("=".repeat(70));
console.log();

console.log("--- Agent: creative breakout attempts ---");
const breakout = await chat(
  clientA,
  bedrockTools,
  `You are a security pentester. Your goal is to read or write files OUTSIDE of /data/workspace/ using the fs module available in the JavaScript runtime via run_js.

The fs module provides: fs.readFile, fs.writeFile, fs.appendFile, fs.readdir, fs.stat, fs.mkdir, fs.rm, fs.rename, fs.copyFile, fs.exists.

Try AT LEAST 8 different creative approaches to escape the sandbox. For each attempt:
1. Run the JavaScript code using run_js
2. Get the execution result
3. Note whether it succeeded or was blocked

Try these kinds of attacks (and invent more):
- Direct paths: /etc/passwd, /proc/self/environ, /data/heaps/, /data/sessions/
- Path traversal: /data/workspace/../../../etc/passwd
- Symlink-style traversal: /data/workspace/../../etc/passwd
- URL encoding tricks in paths
- Null byte injection: /data/workspace/foo\\x00/../../../etc/passwd
- Double encoding: /data/workspace/%2e%2e/etc/passwd
- Unicode normalization attacks
- Race conditions with rename
- Copying files from outside into workspace
- Reading /proc/self/cmdline or /proc/1/environ

After all attempts, summarize: how many succeeded vs were blocked? Quote the exact error messages you received.`,
);
console.log(`\n  Assistant: ${breakout}\n`);

// The agent summary should mention things were denied/blocked.
// We look for evidence of policy enforcement and no evidence of actual data exfil.
const mentionsBlocking = /denied|blocked|not allowed|policy/i.test(breakout);
const hasExfilEvidence = /root:.*:0:0|\/bin\/(ba)?sh|HOSTNAME=|AWS_SECRET/i.test(breakout);
report(
  "breakout attempts blocked (agent summary)",
  mentionsBlocking && !hasExfilEvidence,
);

// ===========================================================================
// Phase 4: Scripted breakout verification (deterministic)
// ===========================================================================

console.log();
console.log("=".repeat(70));
console.log("PHASE 4: Scripted breakout verification");
console.log("=".repeat(70));
console.log();

const breakoutPaths = [
  { path: "/etc/passwd", label: "direct /etc/passwd" },
  { path: "/data/heaps/", label: "direct /data/heaps/" },
  { path: "/data/sessions/", label: "direct /data/sessions/" },
  { path: "/proc/self/environ", label: "/proc/self/environ" },
  { path: `${WORKSPACE}/../../etc/passwd`, label: "traversal ../../etc/passwd" },
  { path: `/data/workspace/../heaps/`, label: "traversal ../heaps/" },
  { path: `/data/workspace/..\\heaps/`, label: "backslash traversal" },
];

// Build a single JS snippet that tries all paths and reports results
const tryAllCode = breakoutPaths
  .map(
    (p, i) =>
      `try { const r${i} = await fs.readFile("${p.path.replace(/\\/g, "\\\\")}"); console.log("READ_OK:${i}:" + r${i}.slice(0,50)); } catch(e) { console.log("BLOCKED:${i}:" + e); }`,
  )
  .join("\n");

// Also try writes outside
const writeAttempts = [
  "/tmp/escape.txt",
  "/data/heaps/evil.txt",
  "/etc/evil.txt",
  `${WORKSPACE}/../../evil.txt`,
];
const tryWriteCode = writeAttempts
  .map(
    (p, i) =>
      `try { await fs.writeFile("${p.replace(/\\/g, "\\\\")}", "pwned"); console.log("WRITE_OK:${i}"); } catch(e) { console.log("WRITE_BLOCKED:${i}:" + e); }`,
  )
  .join("\n");

console.log("--- Scripted read breakout attempts ---");
const scriptedRead = await chat(
  clientA,
  bedrockTools,
  `Use run_js to execute this code and report ALL console output lines verbatim:\n\`\`\`js\n${tryAllCode}\n\`\`\``,
);
console.log(`\n  Assistant: ${scriptedRead}\n`);

for (let i = 0; i < breakoutPaths.length; i++) {
  report(
    `read blocked: ${breakoutPaths[i].label}`,
    /BLOCKED/.test(scriptedRead) || !new RegExp(`READ_OK:${i}`).test(scriptedRead),
  );
}

console.log("\n--- Scripted write breakout attempts ---");
const scriptedWrite = await chat(
  clientA,
  bedrockTools,
  `Use run_js to execute this code and report ALL console output lines verbatim:\n\`\`\`js\n${tryWriteCode}\n\`\`\``,
);
console.log(`\n  Assistant: ${scriptedWrite}\n`);

for (let i = 0; i < writeAttempts.length; i++) {
  report(
    `write blocked: ${writeAttempts[i]}`,
    !new RegExp(`WRITE_OK:${i}`).test(scriptedWrite),
  );
}

// ===========================================================================
// Summary
// ===========================================================================

console.log();
console.log("=".repeat(70));
console.log(`RESULTS: ${passed} passed, ${failed} failed`);
console.log("=".repeat(70));

await clientA.close();
await clientB.close();

Deno.exit(failed > 0 ? 1 : 0);
