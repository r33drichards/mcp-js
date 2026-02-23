import http from "k6/http";
import { check, sleep } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";

// ── Custom metrics ──────────────────────────────────────────────────────
const jsExecDuration = new Trend("js_exec_duration", true);
const jsExecSuccess = new Rate("js_exec_success");
const jsExecCount = new Counter("js_exec_count");
const initDuration = new Trend("mcp_init_duration", true);

// ── Configuration from environment ──────────────────────────────────────
const TARGET_URL = __ENV.TARGET_URL || "http://localhost:8080";
const TARGET_RATE = parseInt(__ENV.TARGET_RATE || "1000");
const DURATION = __ENV.DURATION || "60s";
const TOPOLOGY = __ENV.TOPOLOGY || "unknown";

// ── Scenario configuration ──────────────────────────────────────────────
// The workflow sets TARGET_RATE to 1000, 10000, or 100000.
// We use constant-arrival-rate to attempt the target req/s regardless of
// response time.  Pre-allocated VUs are set generously so k6 has enough
// goroutines to keep up.
export const options = {
  scenarios: {
    mcp_load: {
      executor: "constant-arrival-rate",
      rate: TARGET_RATE,
      timeUnit: "1s",
      duration: DURATION,
      // Start with enough VUs; each VU handles one request at a time.
      // For high rates we need many VUs since each request blocks on I/O.
      preAllocatedVUs: Math.min(TARGET_RATE * 2, 50000),
      maxVUs: Math.min(TARGET_RATE * 4, 100000),
    },
  },
  thresholds: {
    js_exec_success: ["rate>0.90"],       // >=90% success
    js_exec_duration: ["p(95)<10000"],    // p95 < 10s
  },
  tags: {
    topology: TOPOLOGY,
    target_rate: String(TARGET_RATE),
  },
};

// ── MCP JSON-RPC payloads ───────────────────────────────────────────────

function makeInitPayload(id) {
  return JSON.stringify({
    jsonrpc: "2.0",
    id: id,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "k6-loadtest", version: "1.0.0" },
    },
  });
}

function makeToolCallPayload(id, code) {
  return JSON.stringify({
    jsonrpc: "2.0",
    id: id,
    method: "tools/call",
    params: {
      name: "run_js",
      arguments: { code: code },
    },
  });
}

// A set of lightweight JS snippets to execute.  We rotate through them to
// avoid any caching effects while keeping execution cost low (we want to
// measure throughput, not V8 compute time).
const JS_SNIPPETS = [
  "1 + 1",
  "Math.sqrt(144)",
  "JSON.stringify({a: 1, b: 2})",
  "Array.from({length: 10}, (_, i) => i * i).reduce((a, b) => a + b, 0)",
  "'hello'.repeat(3)",
  "Date.now()",
  "Object.keys({x: 1, y: 2, z: 3}).length",
  "[1,2,3,4,5].filter(n => n % 2 === 0).map(n => n * 10)",
];

const JSON_HEADERS = { "Content-Type": "application/json" };

// ── Main test function ──────────────────────────────────────────────────
// Each iteration: initialize a session, then execute one JS snippet.

export default function () {
  const reqId = Math.floor(Math.random() * 1e9);
  const snippet = JS_SNIPPETS[Math.floor(Math.random() * JS_SNIPPETS.length)];

  // 1. Initialize MCP session
  const initRes = http.post(
    `${TARGET_URL}/mcp`,
    makeInitPayload(reqId),
    { headers: JSON_HEADERS, tags: { phase: "init" } }
  );

  initDuration.add(initRes.timings.duration);

  if (initRes.status !== 200) {
    jsExecSuccess.add(false);
    return;
  }

  // Extract session header (rmcp uses Mcp-Session-Id)
  const sessionId =
    initRes.headers["Mcp-Session-Id"] ||
    initRes.headers["mcp-session-id"] ||
    "";

  // 2. Execute JS via tools/call
  const execHeaders = Object.assign({}, JSON_HEADERS);
  if (sessionId) {
    execHeaders["Mcp-Session-Id"] = sessionId;
  }

  const execRes = http.post(
    `${TARGET_URL}/mcp`,
    makeToolCallPayload(reqId + 1, snippet),
    { headers: execHeaders, tags: { phase: "exec" } }
  );

  jsExecDuration.add(execRes.timings.duration);
  jsExecCount.add(1);

  const ok = check(execRes, {
    "status 200": (r) => r.status === 200,
    "has result": (r) => {
      try {
        const body = JSON.parse(r.body);
        // MCP responses can be direct JSON-RPC or SSE-wrapped
        return body.result !== undefined || body.id !== undefined;
      } catch (e) {
        // SSE responses start with "event:" – still valid
        return r.body && r.body.length > 0;
      }
    },
  });

  jsExecSuccess.add(ok);
}

// ── Summary ─────────────────────────────────────────────────────────────

export function handleSummary(data) {
  const summary = {
    topology: TOPOLOGY,
    target_rate: TARGET_RATE,
    duration: DURATION,
    metrics: {
      js_exec_count:
        data.metrics.js_exec_count
          ? data.metrics.js_exec_count.values.count
          : 0,
      js_exec_success_rate:
        data.metrics.js_exec_success
          ? data.metrics.js_exec_success.values.rate
          : 0,
      js_exec_duration_avg:
        data.metrics.js_exec_duration
          ? data.metrics.js_exec_duration.values.avg
          : 0,
      js_exec_duration_p95:
        data.metrics.js_exec_duration
          ? data.metrics.js_exec_duration.values["p(95)"]
          : 0,
      js_exec_duration_p99:
        data.metrics.js_exec_duration
          ? data.metrics.js_exec_duration.values["p(99)"]
          : 0,
      http_req_duration_avg:
        data.metrics.http_req_duration
          ? data.metrics.http_req_duration.values.avg
          : 0,
      http_req_duration_p95:
        data.metrics.http_req_duration
          ? data.metrics.http_req_duration.values["p(95)"]
          : 0,
      http_reqs_per_sec:
        data.metrics.http_reqs
          ? data.metrics.http_reqs.values.rate
          : 0,
      iterations_per_sec:
        data.metrics.iterations
          ? data.metrics.iterations.values.rate
          : 0,
      vus_max:
        data.metrics.vus_max
          ? data.metrics.vus_max.values.max
          : 0,
      dropped_iterations:
        data.metrics.dropped_iterations
          ? data.metrics.dropped_iterations.values.count
          : 0,
    },
  };

  const filename = `results-${TOPOLOGY}-${TARGET_RATE}rps.json`;
  const output = {};
  output[filename] = JSON.stringify(summary, null, 2);
  output["stdout"] = textSummary(data, { indent: "  ", enableColors: true });
  return output;
}

function textSummary(data, opts) {
  // k6 built-in text summary
  let out = `\n${"=".repeat(60)}\n`;
  out += `  Load Test Results: ${TOPOLOGY} @ ${TARGET_RATE} req/s\n`;
  out += `${"=".repeat(60)}\n\n`;

  const m = data.metrics;
  if (m.js_exec_count) {
    out += `  JS Executions:      ${m.js_exec_count.values.count}\n`;
  }
  if (m.js_exec_success) {
    out += `  Success Rate:       ${(m.js_exec_success.values.rate * 100).toFixed(1)}%\n`;
  }
  if (m.iterations) {
    out += `  Iterations/sec:     ${m.iterations.values.rate.toFixed(1)}\n`;
  }
  if (m.http_reqs) {
    out += `  HTTP Reqs/sec:      ${m.http_reqs.values.rate.toFixed(1)}\n`;
  }
  if (m.js_exec_duration) {
    const d = m.js_exec_duration.values;
    out += `  Exec Duration:\n`;
    out += `    avg:  ${d.avg.toFixed(2)} ms\n`;
    out += `    p50:  ${d["p(50)"] ? d["p(50)"].toFixed(2) : "N/A"} ms\n`;
    out += `    p95:  ${d["p(95)"].toFixed(2)} ms\n`;
    out += `    p99:  ${d["p(99)"].toFixed(2)} ms\n`;
    out += `    max:  ${d.max.toFixed(2)} ms\n`;
  }
  if (m.dropped_iterations) {
    out += `  Dropped Iterations: ${m.dropped_iterations.values.count}\n`;
  }
  if (m.vus_max) {
    out += `  Max VUs Used:       ${m.vus_max.values.max}\n`;
  }

  out += `\n${"=".repeat(60)}\n`;
  return out;
}
