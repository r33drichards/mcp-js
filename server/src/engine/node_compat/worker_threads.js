import { EventEmitter } from 'node:events';
let nextThreadId = 1;

function dataUrlSource(url) {
    const match = /^data:([^,]*),(.*)$/s.exec(url.href);
    if (!match) throw new TypeError('Invalid data URL');
    const [, metadata, payload] = match;
    if (metadata.split(';').includes('base64')) return atob(payload);
    return decodeURIComponent(payload);
}

function workerInvocation(filename, options) {
    const execArgv = Array.from(options.execArgv || [], String);
    if (options.eval) {
        return [...execArgv, '--input-type=module', '--eval', String(filename)];
    }
    if (filename instanceof URL && filename.protocol === 'data:') {
        return [...execArgv, '--input-type=module', '--eval', dataUrlSource(filename)];
    }
    const specifier = filename instanceof URL ? filename.href : String(filename);
    return [...execArgv, specifier, ...Array.from(options.argv || [], String)];
}

const initialProcess = globalThis.process;
export const isMainThread = initialProcess?.env?.MCP_V8_WORKER_THREAD !== '1';
export const threadId = Number(initialProcess?.env?.MCP_V8_WORKER_THREAD_ID || 0);
export const workerData = initialProcess?.env?.MCP_V8_WORKER_DATA
    ? JSON.parse(initialProcess.env.MCP_V8_WORKER_DATA)
    : null;
export const parentPort = null;
export const SHARE_ENV = Symbol.for('nodejs.worker_threads.SHARE_ENV');

export class Worker extends EventEmitter {
    #completion;
    #controlPath;
    #exited = false;
    #terminating = false;

    constructor(filename, options = {}) {
        super();
        if (typeof filename !== 'string' && !(filename instanceof URL)) {
            throw new TypeError('The "filename" argument must be of type string or an instance of URL.');
        }
        if (options === null || typeof options !== 'object') {
            throw new TypeError('The "options" argument must be of type object.');
        }
        if (options.eval !== undefined && typeof options.eval !== 'boolean') {
            throw new TypeError('The "options.eval" property must be of type boolean.');
        }
        if (globalThis.__mcpV8NodeCompatCli !== true) {
            const error = new Error('Worker requires the self-hosted Node compatibility CLI');
            error.code = 'ERR_WORKER_UNSUPPORTED_OPERATION';
            throw error;
        }

        this.threadId = nextThreadId++;
        this.resourceLimits = {};
        this.stdin = null;
        this.stdout = null;
        this.stderr = null;

        const workerArgs = ['--node-compat-cli', ...workerInvocation(filename, options)];
        const environment = {
            MCP_V8_WORKER_THREAD: '1',
            MCP_V8_WORKER_THREAD_ID: String(this.threadId),
            MCP_V8_WORKER_DATA: JSON.stringify(options.workerData ?? null),
        };
        const executable = globalThis.process?.execPath || Deno.core.ops.op_process_exec_path?.() || '/usr/bin/node';
        this.#controlPath = `/tmp/mcp-v8-worker-${Date.now()}-${Math.random()}.pid`;
        const command = new Deno.Command('/bin/sh', {
            args: [
                '-c',
                'printf %s "$$" > "$1"; shift; exec "$@"',
                'mcp-v8-worker',
                this.#controlPath,
                executable,
                ...workerArgs,
            ],
            env: environment,
        });
        this.#completion = command.output().then((output) => {
            this.#exited = true;
            const stderr = new TextDecoder().decode(output.stderr).trim();
            if (!output.success && !this.#terminating) {
                const error = new Error(stderr || `Worker stopped with exit code ${output.code}`);
                error.code = output.code;
                this.emit('error', error);
            }
            this.emit('exit', output.code);
            return output.code;
        }, (error) => {
            this.#exited = true;
            this.emit('error', error);
            this.emit('exit', 1);
            return 1;
        });
    }

    ref() { return this; }
    unref() { return this; }
    async terminate() {
        if (!this.#exited) {
            this.#terminating = true;
            const kill = new Deno.Command('/bin/sh', {
                args: [
                    '-c',
                    'i=0; while [ ! -s "$1" ] && [ "$i" -lt 100 ]; do sleep 0.01; i=$((i+1)); done; [ ! -s "$1" ] || kill -TERM "$(cat "$1")" 2>/dev/null || true',
                    'mcp-v8-worker-kill',
                    this.#controlPath,
                ],
            });
            await kill.output();
        }
        return this.#completion;
    }
}

export default {
    Worker,
    isMainThread,
    parentPort,
    SHARE_ENV,
    threadId,
    workerData,
};
