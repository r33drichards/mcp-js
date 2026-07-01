// Drive the Codex `app-server` from inside mcp-v8's `run_js` runtime.
//
// Requires:
//   • the interactive subprocess support added to mcp-v8 (Deno.Command().spawn())
//   • the Codex binary on PATH (or set CODEX_BIN below to an absolute path)
//   • a subprocess policy that allows spawning it — see
//     ../../policies/codex-app-server.rego
//
// It performs the app-server handshake, creates a thread, and starts a
// "hello world" turn. With no Codex auth configured, inference fails (401),
// but the thread is created and persisted to a rollout file on disk — exactly
// the "thread created, inference errored" outcome.
//
// The result of this script is a summary object (returned as the run_js value).

const CODEX_BIN = "codex"; // or an absolute path, e.g. the npm vendor binary
const CODEX_HOME = "/tmp/codex-mcpjs-home";
const WORKDIR = "/tmp/codex-mcpjs-work";

async function main() {
  const child = new Deno.Command(CODEX_BIN, {
    args: ["app-server"],
    env: { CODEX_HOME, RUST_LOG: "error" },
  }).spawn();

  const transcript = [];

  // Read stdout lines until we get a JSON-RPC message matching `predicate`.
  // Every line seen is recorded in `transcript`.
  async function readUntil(predicate, maxLines) {
    for (let i = 0; i < (maxLines || 200); i++) {
      const line = await child.readLine();
      if (line === null) return null; // EOF
      let msg;
      try { msg = JSON.parse(line); } catch { continue; }
      transcript.push(msg.method ? { notify: msg.method } : { response: msg.id });
      if (predicate(msg)) return msg;
    }
    return null;
  }

  // 1. initialize
  await child.writeLine(JSON.stringify({
    method: "initialize", id: 0,
    params: { clientInfo: { name: "mcp_v8", title: "mcp-v8", version: "0.1.0" }, capabilities: null },
  }));
  await readUntil((m) => m.id === 0, 20);

  // 2. initialized (notification)
  await child.writeLine(JSON.stringify({ method: "initialized" }));

  // 3. thread/start
  await child.writeLine(JSON.stringify({ method: "thread/start", id: 1, params: { cwd: WORKDIR } }));
  const started = await readUntil((m) => m.id === 1, 40);
  const threadId = started.result.thread.id;
  const rolloutPath = started.result.thread.path;
  console.log("thread created:", threadId);
  console.log("rollout path:", rolloutPath);

  // 4. turn/start "hello world" — inference is expected to fail without auth.
  await child.writeLine(JSON.stringify({
    method: "turn/start", id: 2,
    params: { threadId, input: [{ type: "text", text: "hello world", text_elements: [] }] },
  }));

  // Wait for the first inference error (or a turn terminal), then stop.
  const outcome = await readUntil((m) =>
    m.method === "error" ||
    m.method === "turn/failed" ||
    m.method === "turn/completed" ||
    (m.id === 2 && m.error), 80);

  const inferenceError = outcome && outcome.method === "error"
    ? (outcome.params && outcome.params.error)
    : (outcome && outcome.error) || null;

  await child.kill();

  return {
    threadId,
    rolloutPath,
    threadCreated: !!threadId,
    inferenceError,             // e.g. 401 responseStreamDisconnected (no auth)
    transcript,
  };
}

// run_js returns the value of the final expression.
await main();
