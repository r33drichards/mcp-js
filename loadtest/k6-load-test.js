import http from "k6/http";
import { check } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";

// ── Custom metrics ──────────────────────────────────────────────────────
const jsExecDuration = new Trend("js_exec_duration", true);
const jsExecSuccess = new Rate("js_exec_success");
const jsExecCount = new Counter("js_exec_count");

// ── Configuration from environment ──────────────────────────────────────
// TARGET_URLS: comma-separated list of base URLs (for cluster round-robin)
const TARGET_URLS = (__ENV.TARGET_URLS || __ENV.TARGET_URL || "http://localhost:3001")
  .split(",")
  .map((u) => u.trim());
const TARGET_RATE = parseInt(__ENV.TARGET_RATE || "1000");
const DURATION = __ENV.DURATION || "60s";
const TOPOLOGY = __ENV.TOPOLOGY || "unknown";

// ── Scenario configuration ──────────────────────────────────────────────
// constant-arrival-rate attempts exactly TARGET_RATE iterations/sec
// regardless of response time.
export const options = {
  scenarios: {
    mcp_load: {
      executor: "constant-arrival-rate",
      rate: TARGET_RATE,
      timeUnit: "1s",
      duration: DURATION,
      preAllocatedVUs: Math.min(Math.ceil(TARGET_RATE * 0.1), 500),
      maxVUs: Math.min(TARGET_RATE, 5000),
    },
  },
  thresholds: {
    js_exec_success: ["rate>0.90"],
    js_exec_duration: ["p(95)<10000"],
  },
  tags: {
    topology: TOPOLOGY,
    target_rate: String(TARGET_RATE),
  },
};

// Lightweight JS snippets — rotate to avoid caching effects.
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

const HEADERS = { "Content-Type": "application/json" };

// Simple round-robin counter for distributing across cluster nodes.
let rrCounter = 0;

function pickUrl() {
  const url = TARGET_URLS[rrCounter % TARGET_URLS.length];
  rrCounter++;
  return url;
}

// ── Main test function ──────────────────────────────────────────────────
// Each iteration: single POST /api/exec with JSON body
export default function () {
  const baseUrl = pickUrl();
  const snippet = JS_SNIPPETS[Math.floor(Math.random() * JS_SNIPPETS.length)];

  const payload = JSON.stringify({ code: snippet });

  const res = http.post(`${baseUrl}/api/exec`, payload, {
    headers: HEADERS,
    timeout: "10s",
  });

  jsExecDuration.add(res.timings.duration);
  jsExecCount.add(1);

  const ok = check(res, {
    "status 200": (r) => r.status === 200,
    "has output": (r) => {
      try {
        const body = JSON.parse(r.body);
        return body.output !== undefined && !String(body.output).startsWith("Error:");
      } catch (_) {
        return false;
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
    target_urls: TARGET_URLS,
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
  output["stdout"] = textSummary(data);
  return output;
}

function safeFixed(val, digits) {
  return val != null ? val.toFixed(digits) : "N/A";
}

function textSummary(data) {
  let out = `\n${"=".repeat(60)}\n`;
  out += `  Load Test Results: ${TOPOLOGY} @ ${TARGET_RATE} req/s\n`;
  out += `  Target URLs: ${TARGET_URLS.join(", ")}\n`;
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
    out += `    avg:  ${safeFixed(d.avg, 2)} ms\n`;
    out += `    p50:  ${safeFixed(d["p(50)"], 2)} ms\n`;
    out += `    p95:  ${safeFixed(d["p(95)"], 2)} ms\n`;
    out += `    p99:  ${safeFixed(d["p(99)"], 2)} ms\n`;
    out += `    max:  ${safeFixed(d.max, 2)} ms\n`;
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
