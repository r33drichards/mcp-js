#!/usr/bin/env -S deno run -A

const KEYCLOAK_URL = getEnv("KEYCLOAK_URL", "http://localhost:8080");
const MCP_URL = getEnv("MCP_SERVER_URL", "http://localhost:3000");
const OPA_URL = getEnv("OPA_URL", "http://localhost:8181");
const UPSTREAM_BIND_HOST = getEnv("PROTECTED_UPSTREAM_BIND_HOST", "0.0.0.0");
const UPSTREAM_HOST_FOR_MCP = getEnv(
  "PROTECTED_UPSTREAM_HOST",
  "host.docker.internal",
);
const UPSTREAM_PORT = Number.parseInt(
  getEnv("PROTECTED_UPSTREAM_PORT", "4010"),
  10,
);
const UPSTREAM_PATH = getEnv("PROTECTED_UPSTREAM_PATH", "/protected");
const REALM = getEnv("KEYCLOAK_REALM", "mcp");
const EXPECTED_AZP = getEnv("KEYCLOAK_CLIENT_ID", "mcp-client");

function getEnv(name, fallback) {
  if (typeof Deno === "undefined") {
    return fallback;
  }
  return Deno.env.get(name) ?? fallback;
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function decodeBase64Url(input) {
  const normalized = input.replace(/-/g, "+").replace(/_/g, "/");
  const padding = "=".repeat((4 - (normalized.length % 4)) % 4);
  const padded = normalized + padding;

  if (typeof atob === "function") {
    const binary = atob(padded);
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  }

  return Buffer.from(padded, "base64").toString("utf8");
}

export function extractBearerToken(authorization) {
  assert(typeof authorization === "string", "missing Authorization header");
  const match = authorization.match(/^Bearer\s+(.+)$/i);
  assert(match, `expected a Bearer token, got: ${authorization}`);
  return match[1];
}

export function decodeJwtPayload(token) {
  const parts = token.split(".");
  assert(parts.length >= 2, "JWT must have at least two sections");
  return JSON.parse(decodeBase64Url(parts[1]));
}

export function parseExecutionResult(body) {
  const execution = typeof body === "string" ? JSON.parse(body) : body;
  assert(execution.status, "execution response missing status");
  if (execution.status === "failed" || execution.status === "timed_out") {
    throw new Error(
      `execution ${execution.status}: ${execution.error ?? "unknown error"}`,
    );
  }
  assert(
    execution.status === "completed",
    `execution did not complete: ${execution.status}`,
  );
  return execution;
}

export async function waitForServiceReady(
  url,
  options = {},
) {
  const {
    timeoutMs = 30_000,
    intervalMs = 250,
    fetchImpl = fetch,
  } = options;
  const deadline = Date.now() + timeoutMs;
  let lastError = "service did not respond";

  while (Date.now() < deadline) {
    try {
      const response = await fetchImpl(url);
      if (response.ok) {
        return;
      }
      lastError = `received HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }

    await sleep(intervalMs);
  }

  throw new Error(`service not ready at ${url}: ${lastError}`);
}

export async function waitForMcpReady(mcpUrl, options = {}) {
  await waitForServiceReady(`${mcpUrl}/api/executions`, options);
}

export async function waitForOpaReady(opaUrl, options = {}) {
  await waitForServiceReady(`${opaUrl}/health`, options);
}

export async function waitForKeycloakRealmReady(
  keycloakUrl,
  realm,
  options = {},
) {
  await waitForServiceReady(
    `${keycloakUrl}/realms/${realm}/.well-known/openid-configuration`,
    options,
  );
}

export async function submitExecution(mcpUrl, code) {
  const response = await fetch(`${mcpUrl}/api/exec`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });

  if (!response.ok) {
    throw new Error(
      `POST /api/exec failed (${response.status}): ${await response.text()}`,
    );
  }

  const body = await response.json();
  assert(
    typeof body.execution_id === "string" && body.execution_id.length > 0,
    "execution_id missing from /api/exec response",
  );
  return body.execution_id;
}

export async function waitForExecution(mcpUrl, executionId, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    const response = await fetch(`${mcpUrl}/api/executions/${executionId}`);
    if (!response.ok) {
      throw new Error(
        `GET /api/executions/${executionId} failed (${response.status}): ${await response.text()}`,
      );
    }

    const body = await response.json();
    if (body.status !== "running") {
      return body;
    }

    await sleep(100);
  }

  throw new Error(`execution ${executionId} did not finish within ${timeoutMs}ms`);
}

export async function readExecutionOutput(mcpUrl, executionId) {
  const response = await fetch(`${mcpUrl}/api/executions/${executionId}/output`);
  if (!response.ok) {
    throw new Error(
      `GET /api/executions/${executionId}/output failed (${response.status}): ${await response.text()}`,
    );
  }
  return await response.json();
}

export function parseJsonFromConsoleOutput(body) {
  const output = typeof body === "string" ? JSON.parse(body) : body;
  assert(typeof output.data === "string", "execution output missing data");
  const lines = output.data
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  assert(lines.length > 0, "execution output did not contain any console lines");
  return JSON.parse(lines[lines.length - 1]);
}

export async function runProtectedFetch(mcpUrl, protectedUrl) {
  const code = `
    (async () => {
      const resp = await fetch(${JSON.stringify(protectedUrl)});
      const body = await resp.text();
      if (!resp.ok) {
        throw new Error("protected upstream returned " + resp.status + ": " + body);
      }
      console.log(body);
    })()
  `;

  const executionId = await submitExecution(mcpUrl, code);
  const execution = await waitForExecution(mcpUrl, executionId);
  parseExecutionResult(execution);
  const output = await readExecutionOutput(mcpUrl, executionId);
  return parseJsonFromConsoleOutput(output);
}

async function startProtectedUpstream(bindHost, port, path) {
  assert(typeof Deno !== "undefined", "this script must run under Deno");

  const requests = [];
  const controller = new AbortController();
  const server = Deno.serve(
    { hostname: bindHost, port, signal: controller.signal },
    async (request) => {
      const url = new URL(request.url);
      if (url.pathname !== path) {
        return new Response("not found", { status: 404 });
      }

      const record = {
        method: request.method,
        path: url.pathname,
        authorization: request.headers.get("authorization"),
        userAgent: request.headers.get("user-agent"),
        requestNumber: requests.length + 1,
      };
      requests.push(record);

      const status = record.authorization ? 200 : 401;
      return new Response(JSON.stringify(record), {
        status,
        headers: { "content-type": "application/json" },
      });
    },
  );

  await sleep(50);

  return {
    requests,
    close: async () => {
      controller.abort();
      await server.finished.catch(() => undefined);
    },
  };
}

function reportPass(message) {
  console.log(`PASS: ${message}`);
}

function reportDetail(label, value) {
  console.log(`${label}: ${value}`);
}

async function main() {
  const protectedUrl = `http://${UPSTREAM_HOST_FOR_MCP}:${UPSTREAM_PORT}${UPSTREAM_PATH}`;

  reportDetail("Keycloak", KEYCLOAK_URL);
  reportDetail("OPA", OPA_URL);
  reportDetail("mcp-js", MCP_URL);
  reportDetail("Protected upstream", protectedUrl);
  console.log();

  const upstream = await startProtectedUpstream(
    UPSTREAM_BIND_HOST,
    UPSTREAM_PORT,
    UPSTREAM_PATH,
  );

  try {
    reportDetail(
      "Waiting for Keycloak realm readiness",
      `${KEYCLOAK_URL}/realms/${REALM}/.well-known/openid-configuration`,
    );
    await waitForKeycloakRealmReady(KEYCLOAK_URL, REALM);
    reportPass("Keycloak realm is ready");

    reportDetail("Waiting for OPA readiness", `${OPA_URL}/health`);
    await waitForOpaReady(OPA_URL);
    reportPass("OPA is ready");

    reportDetail("Waiting for mcp-js readiness", `${MCP_URL}/api/executions`);
    await waitForMcpReady(MCP_URL);
    reportPass("mcp-js HTTP API is ready");

    const first = await runProtectedFetch(MCP_URL, protectedUrl);
    const second = await runProtectedFetch(MCP_URL, protectedUrl);

    const firstToken = extractBearerToken(first.authorization);
    const secondToken = extractBearerToken(second.authorization);
    const claims = decodeJwtPayload(firstToken);

    assert(
      claims.azp === EXPECTED_AZP,
      `expected azp=${EXPECTED_AZP}, got ${String(claims.azp)}`,
    );
    assert(
      typeof claims.iss === "string" &&
        claims.iss.includes(`/realms/${REALM}`),
      `unexpected iss claim: ${String(claims.iss)}`,
    );
    assert(typeof claims.exp === "number", "token missing exp claim");

    reportPass("protected upstream received a Keycloak-issued bearer token");
    reportDetail("Observed azp", String(claims.azp));
    reportDetail("Observed iss", String(claims.iss));

    assert(
      firstToken === secondToken,
      "expected the server to reuse the cached token before expiry",
    );
    reportPass("token was reused across repeated fetches before expiry");

    assert(
      upstream.requests.length >= 2,
      `expected at least 2 upstream requests, saw ${upstream.requests.length}`,
    );
    reportPass("secure-session compose environment served repeated fetches end to end");
  } finally {
    await upstream.close();
  }
}

if (import.meta.main) {
  await main();
}
