import { HostToolRun, type Engine } from "./index.js";

export interface HostTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

export interface ToolRuntimeConfig {
  server: string;
  tools: readonly HostTool[];
  call(name: string, args: Record<string, unknown>, signal: AbortSignal): unknown | Promise<unknown>;
}

export interface ToolRuntimeResult {
  status: string;
  output?: string;
  output_truncated?: boolean;
  error?: string;
  wait_timed_out?: boolean;
  execution_id: string;
}

/** Bind a host's tool authority once and allocate a separate channel for each execution. */
export function createToolRuntime(engine: Engine, config: ToolRuntimeConfig) {
  const { server, call } = config;
  const definitions = config.tools.map(tool => ({
    name: tool.name,
    description: tool.description,
    inputSchemaJson: JSON.stringify(tool.inputSchema),
  }));

  return {
    listTools() {
      return definitions.map(tool => ({
        name: `runjs__${server}__${tool.name}`,
        description: `Discovery only. Use run_js with await mcp.callTool(${JSON.stringify(server)}, ${JSON.stringify(tool.name)}, args).\n\n${tool.description}`,
        inputSchema: JSON.parse(tool.inputSchemaJson) as Record<string, unknown>,
      }));
    },
    async runJs(code: string, signal?: AbortSignal): Promise<ToolRuntimeResult> {
      signal?.throwIfAborted();
      const channel = HostToolRun.create(server, definitions);
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
              payload = JSON.stringify(await call(name, JSON.parse(argumentsJson), controller.signal)) ?? "null";
            } catch (error) {
              payload = error instanceof Error ? error.message : String(error);
              failed = true;
            }
            if (!controller.signal.aborted) channel.respond(id, payload, failed);
          })();
        }
      }

      const serving = serve();
      try {
        const run = engine.runHostJs(code, channel);
        const result = await Promise.race([run, serving.then(() => run)]);
        signal?.throwIfAborted();
        return JSON.parse(result) as ToolRuntimeResult;
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
