import { HostToolRun, hostToolStubInstructions, hostToolStubs, type EngineLike } from "./index.js";

export interface HostTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

export interface ToolRuntimeConfig {
  server: string;
  tools: readonly HostTool[];
  stubs?: boolean;
  stubPrefix?: string;
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
export function createToolRuntime(engine: EngineLike, config: ToolRuntimeConfig) {
  const { server, call } = config;
  const definitions = config.tools.map(tool => ({
    name: tool.name,
    description: tool.description,
    inputSchemaJson: JSON.stringify(tool.inputSchema),
  }));

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
