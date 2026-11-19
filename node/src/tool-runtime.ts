import { HostToolRun, ToolCallRequestBuilder, hostToolStubInstructions, hostToolStubs, type EngineLike } from "./index.js";

export interface HostTool {
  description: string;
  inputSchema: Record<string, unknown>;
  run(args: Record<string, unknown>, signal: AbortSignal): unknown;
}

type RegisteredTool = Readonly<Omit<HostTool, "inputSchema"> & { inputSchemaJson: string }>;

export interface ToolRuntimeConfig {
  readonly server: string;
  readonly tools: ReadonlyMap<string, RegisteredTool>;
  readonly stubs: boolean;
  readonly stubPrefix?: string;
}

/** Assemble a reusable configuration with immutable setters, like EngineConfigBuilder. */
export class ToolRuntimeBuilder {
  #config: ToolRuntimeConfig = { server: "", tools: new Map(), stubs: true };

  #with(changes: Partial<ToolRuntimeConfig>): ToolRuntimeBuilder {
    const builder = new ToolRuntimeBuilder();
    builder.#config = { ...this.#config, ...changes };
    return builder;
  }

  server(name: string): ToolRuntimeBuilder {
    return this.#with({ server: name });
  }

  registerTool(name: string, tool: HostTool): ToolRuntimeBuilder {
    if (!name || this.#config.tools.has(name)) {
      throw new Error(`Host tool name must be nonempty and unique: ${name}`);
    }
    const tools = new Map(this.#config.tools);
    tools.set(name, Object.freeze({
      description: tool.description,
      inputSchemaJson: JSON.stringify(tool.inputSchema),
      run: tool.run,
    }));
    return this.#with({ tools });
  }

  stubs(enabled: boolean): ToolRuntimeBuilder {
    return this.#with({ stubs: enabled });
  }

  stubPrefix(prefix: string): ToolRuntimeBuilder {
    return this.#with({ stubPrefix: prefix });
  }

  build(): ToolRuntimeConfig {
    if (!this.#config.server || this.#config.server.includes("__")) {
      throw new Error("Host tool server must be nonempty and cannot contain '__'");
    }
    return { ...this.#config, tools: new Map(this.#config.tools) };
  }
}

export interface ToolRuntimeResult {
  status?: string;
  output?: string;
  output_truncated?: boolean;
  error?: string;
  wait_timed_out?: boolean;
  execution_id?: string;
}

/** Bind a host's tool authority once and allocate a separate channel for each execution. */
export function createToolRuntime(engine: EngineLike, config: ToolRuntimeConfig) {
  const { server } = config;
  const handlers = new Map<string, HostTool["run"]>();
  const definitions = Array.from(config.tools, ([name, { description, inputSchemaJson, run }]) => {
    handlers.set(name, run);
    return { name, description, inputSchemaJson };
  });

  const exposedStubs = config.stubs ? hostToolStubs(server, definitions, config.stubPrefix) : [];
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
              payload = error instanceof Error ? error.message : String(error);
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
