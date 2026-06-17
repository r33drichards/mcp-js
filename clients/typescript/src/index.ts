
import createOpenApiClient from "openapi-fetch";
import type { components, paths } from "./schema";

export type { components, paths } from "./schema";

export type ExecRequest = components["schemas"]["ExecRequest"];
export type ExecAccepted = components["schemas"]["ExecAccepted"];
export type ExecutionInfo = components["schemas"]["ExecutionInfo"];
export type ExecutionOutput = components["schemas"]["ExecutionOutput"];
export type CancelResult = components["schemas"]["CancelResult"];


export type ExecutionStatus =
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "timed_out";

const TERMINAL_STATUSES = new Set<string>([
  "completed",
  "failed",
  "cancelled",
  "timed_out",
]);

export function isTerminalStatus(status: string): boolean {
  return TERMINAL_STATUSES.has(status);
}

export interface McpV8ClientOptions {
  
  baseUrl: string;
  
  headers?: Record<string, string>;
  
  fetch?: typeof fetch;
}

export interface RunJsOptions {
  
  heap?: string;
  
  session?: string;
  
  heapMemoryMaxMb?: number;
  
  executionTimeoutSecs?: number;
  
  tags?: Record<string, string>;
  
  pollIntervalMs?: number;
  
  signal?: AbortSignal;
}


export interface RunJsResult {
  executionId: string;
  status: ExecutionStatus;
  
  output: string;
  
  error?: string;
  
  result?: string;
  
  heap?: string;
}

export class McpV8Error extends Error {
  readonly status?: number;
  constructor(message: string, status?: number) {
    super(message);
    this.name = "McpV8Error";
    this.status = status;
  }
}

const delay = (ms: number, signal?: AbortSignal): Promise<void> =>
  new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason ?? new McpV8Error("Aborted"));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason ?? new McpV8Error("Aborted"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });


export class McpV8Client {
  private readonly api: ReturnType<typeof createOpenApiClient<paths>>;

  constructor(options: McpV8ClientOptions) {
    this.api = createOpenApiClient<paths>({
      baseUrl: options.baseUrl.replace(/\/+$/, ""),
      headers: options.headers,
      fetch: options.fetch,
    });
  }

  
  async exec(body: ExecRequest, signal?: AbortSignal): Promise<ExecAccepted> {
    const { data, error, response } = await this.api.POST("/api/exec", {
      body,
      signal,
    });
    if (error || !data) {
      throw new McpV8Error(
        `exec failed: ${formatError(error)}`,
        response.status,
      );
    }
    return data;
  }

  
  async getExecution(id: string, signal?: AbortSignal): Promise<ExecutionInfo> {
    const { data, error, response } = await this.api.GET(
      "/api/executions/{id}",
      { params: { path: { id } }, signal },
    );
    if (error || !data) {
      throw new McpV8Error(
        `getExecution failed: ${formatError(error)}`,
        response.status,
      );
    }
    return data;
  }

  
  async getExecutionOutput(
    id: string,
    query?: paths["/api/executions/{id}/output"]["get"]["parameters"]["query"],
    signal?: AbortSignal,
  ): Promise<ExecutionOutput> {
    const { data, error, response } = await this.api.GET(
      "/api/executions/{id}/output",
      { params: { path: { id }, query }, signal },
    );
    if (error || !data) {
      throw new McpV8Error(
        `getExecutionOutput failed: ${formatError(error)}`,
        response.status,
      );
    }
    return data;
  }

  
  async cancelExecution(id: string, signal?: AbortSignal): Promise<CancelResult> {
    const { data, error, response } = await this.api.POST(
      "/api/executions/{id}/cancel",
      { params: { path: { id } }, signal },
    );
    if (error || !data) {
      throw new McpV8Error(
        `cancelExecution failed: ${formatError(error)}`,
        response.status,
      );
    }
    return data;
  }

  
  async collectOutput(id: string, signal?: AbortSignal): Promise<string> {
    let lineOffset = 0;
    let out = "";
    
    for (let i = 0; i < 100_000; i++) {
      const page = await this.getExecutionOutput(
        id,
        { line_offset: lineOffset },
        signal,
      );
      out += page.data;
      if (!page.has_more) {
        break;
      }
      
      if (page.next_line_offset <= lineOffset) {
        break;
      }
      lineOffset = page.next_line_offset;
    }
    return out;
  }

  
  async runJs(code: string, options: RunJsOptions = {}): Promise<RunJsResult> {
    const { signal } = options;
    const accepted = await this.exec(
      {
        code,
        heap: options.heap ?? null,
        session: options.session ?? null,
        heap_memory_max_mb: options.heapMemoryMaxMb ?? null,
        execution_timeout_secs: options.executionTimeoutSecs ?? null,
        tags: options.tags ?? null,
      },
      signal,
    );
    const id = accepted.execution_id;
    const pollIntervalMs = options.pollIntervalMs ?? 150;

    let info = await this.getExecution(id, signal);
    while (!isTerminalStatus(info.status)) {
      await delay(pollIntervalMs, signal);
      info = await this.getExecution(id, signal);
    }

    const output = await this.collectOutput(id, signal);
    return {
      executionId: id,
      status: info.status as ExecutionStatus,
      output,
      error: info.error ?? undefined,
      result: info.result ?? undefined,
      heap: info.heap ?? undefined,
    };
  }
}


export function createMcpV8Client(
  baseUrl: string,
  options?: Omit<McpV8ClientOptions, "baseUrl">,
): McpV8Client {
  return new McpV8Client({ baseUrl, ...options });
}

function formatError(error: unknown): string {
  if (!error) {
    return "unknown error";
  }
  if (typeof error === "object" && error !== null && "error" in error) {
    return String((error as { error: unknown }).error);
  }
  return typeof error === "string" ? error : JSON.stringify(error);
}
