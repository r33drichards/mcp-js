import { HostToolRun, ToolCallRequestBuilder, hostToolStubInstructions, hostToolStubs, type EngineLike } from "./index.js";

export interface HostTool {
  description: string;
  inputSchema: Record<string, unknown>;
  run(args: Record<string, unknown>, signal: AbortSignal): unknown;
}

export interface ToolRuntimeConfig {
  readonly server: string;
  readonly tools: Readonly<Record<string, HostTool>>;
  readonly stubs?: boolean;
  readonly stubPrefix?: string;
}

export interface ToolRuntimeResult {
  status?: string;
  output?: string;
  output_truncated?: boolean;
  error?: string;
  wait_timed_out?: boolean;
  execution_id?: string;
}

function formatToolError(error: unknown): string {
  try {
    const message = error instanceof Error ? error.message : error;
    return String(message);
  } catch {
    return "Host tool failed";
  }
}

/** Bind a host's tool authority once and allocate a separate channel for each execution. */
export function createToolRuntime(engine: EngineLike, config: ToolRuntimeConfig) {
  const { server } = config;
  const handlers = new Map<string, HostTool["run"]>();
  const definitions = Object.entries(config.tools).map(([name, { description, inputSchema, run }]) => {
    handlers.set(name, run);
    return { name, description, inputSchemaJson: JSON.stringify(inputSchema) };
  });

  const stubs = hostToolStubs(server, definitions, config.stubPrefix);
  const exposedStubs = config.stubs === false ? [] : stubs;
  const stubTargets = new Map(exposedStubs.map((stub, index) => [stub.name, definitions[index].name]));

  return {
    listTools() {
      return exposedStubs.map(stub => ({
        name: stub.name,
        description: stub.description ?? "",
        inputSchema: JSON.parse(stub.inputSchemaJson) as Record<string, unknown>,
      }));
    },
    stubInstructions(name: string, args: Record<string, unknown>): string | undefined {
      const tool = stubTargets.get(name);
      if (tool === undefined) return undefined;
      return hostToolStubInstructions(server, tool, JSON.stringify(args));
    },
    async runJs(code: string, signal?: AbortSignal): Promise<ToolRuntimeResult> {
      signal?.throwIfAborted();
      const channel = HostToolRun.create(server, definitions) as HostToolRun;
      const controller = new AbortController();
      const abort = () => {
        controller.abort();
        channel.close();
      };
      signal?.addEventListener("abort", abort, { once: true });
      async function serve() {
        for (let request = await channel.nextCall(); request; request = await channel.nextCall()) {
          if (controller.signal.aborted) break;
          const { id, name, argumentsJson } = request;
          // Independent calls can be outstanding concurrently, as with Promise.all in guest JS.
          void (async () => {
            let payload: string;
            let failed = false;
            try {
              const handler = handlers.get(name);
              if (!handler) throw new Error(`Unknown host tool: ${name}`);
              const result = await handler(JSON.parse(argumentsJson), controller.signal);
              payload = JSON.stringify(result) ?? "null";
            } catch (error) {
              payload = formatToolError(error);
              failed = true;
            }
            if (!controller.signal.aborted) channel.respond(id, payload, failed);
          })();
        }
      }

      const serving = serve();
      try {
        const request = new ToolCallRequestBuilder()
          .name("run_js")
          .argumentsJson(JSON.stringify({ code, wait: true }))
          .hostTools(channel)
          .build();
        const run = engine.invokeTool(request);
        const result = await Promise.race([run, serving.then(() => run)]);
        signal?.throwIfAborted();
        return JSON.parse(result) as ToolRuntimeResult;
      } catch (error) {
        signal?.throwIfAborted();
        throw error;
      } finally {
        signal?.removeEventListener("abort", abort);
        abort();
        try {
          await serving;
        } finally {
          channel.uniffiDestroy();
        }
      }
    },
  };
}
