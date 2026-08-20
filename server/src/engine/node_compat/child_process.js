import { Buffer } from 'node:buffer';
import { EventEmitter } from 'node:events';

const forkHelper = String.raw`
const fs = require('node:fs');
const { fork } = require('node:child_process');
const [target, argsJson, messageJson, controlPath] = process.argv.slice(1);
const child = fork(target, JSON.parse(argsJson), {
  stdio: ['ignore', 'ignore', 'ignore', 'ipc'],
});
let finished = false;
let hardKillTimer;
function record(value) {
  process.stdout.write(JSON.stringify(value) + '\n');
}
function cleanup() {
  clearInterval(controlPoll);
  clearTimeout(killTimer);
  clearTimeout(hardKillTimer);
  try { fs.unlinkSync(controlPath); } catch {}
}
const controlPoll = setInterval(() => {
  if (!fs.existsSync(controlPath)) return;
  try { fs.unlinkSync(controlPath); } catch {}
  if (child.connected) child.disconnect();
}, 10);
const killTimer = setTimeout(() => {
  if (finished) return;
  child.kill('SIGTERM');
  hardKillTimer = setTimeout(() => {
    if (!finished) child.kill('SIGKILL');
  }, 500);
}, 5000);
child.once('message', (message) => {
  record({ type: 'message', value: message });
  if (child.connected) child.disconnect();
});
child.once('error', (error) => record({ type: 'error', message: error.message }));
child.once('exit', (code, signal) => {
  finished = true;
  cleanup();
  record({ type: 'exit', code, signal });
});
child.send(JSON.parse(messageJson));
`;

function hostPath(filename) {
    const value = String(filename);
    const corpus = globalThis.__NODE_TEST_CORPUS_HOST__;
    return corpus && value.startsWith('/test/') ? corpus + value : value;
}

class ChildProcess extends EventEmitter {
    #modulePath;
    #args;
    #options;
    #started = false;
    #settled = false;
    #controlPath;
    #executable;

    constructor(modulePath, args, options) {
        super();
        this.connected = true;
        this.exitCode = null;
        this.signalCode = null;
        this.#modulePath = hostPath(modulePath);
        this.#args = args;
        this.#options = options;
        this.#controlPath = `/tmp/mcp-node-fork-${Date.now()}-${Math.random()}`;
    }

    send(message, callback) {
        if (!this.connected || this.#started) return false;
        this.#started = true;
        const executable = this.#options.execPath ||
            globalThis.__NODE_TEST_EXEC_PATH__ || globalThis.process?.execPath || 'node';
        this.#executable = executable;
        const command = new Deno.Command(executable, {
            args: [
                '-e', forkHelper, this.#modulePath, JSON.stringify(this.#args),
                JSON.stringify(message), this.#controlPath,
            ],
        });
        command.output().then((output) => {
            const text = new TextDecoder().decode(output.stdout);
            const records = text.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
            const messageRecord = records.find((record) => record.type === 'message');
            const errorRecord = records.find((record) => record.type === 'error');
            const exitRecord = records.find((record) => record.type === 'exit');
            this.#settled = true;
            if (errorRecord) this.emit('error', new Error(errorRecord.message));
            if (messageRecord) this.emit('message', messageRecord.value);
            setTimeout(() => {
                this.exitCode = exitRecord?.code ?? output.code;
                this.signalCode = exitRecord?.signal ?? null;
                this.connected = false;
                this.emit('exit', this.exitCode, this.signalCode);
                this.emit('close', this.exitCode, this.signalCode);
            }, 0);
            callback?.(null);
        }, (error) => {
            this.#settled = true;
            this.connected = false;
            callback?.(error);
            this.emit('error', error);
            setTimeout(() => {
                this.exitCode = -1;
                this.emit('exit', -1, null);
                this.emit('close', -1, null);
            }, 0);
        });
        return true;
    }

    disconnect() {
        if (!this.connected) return this;
        this.connected = false;
        this.emit('disconnect');
        if (this.#started && !this.#settled) {
            const signal = new Deno.Command(this.#executable, {
                args: [
                    '-e',
                    "require('node:fs').writeFileSync(process.argv[1], 'disconnect')",
                    this.#controlPath,
                ],
            });
            signal.output().catch((error) => this.emit('error', error));
        }
        return this;
    }
}

class BufferedReadable extends EventEmitter {
    #encoding;
    #chunks = [];
    #resolve;
    #done = new Promise((resolve) => { this.#resolve = resolve; });

    setEncoding(encoding) {
        this.#encoding = String(encoding);
        return this;
    }

    finish(bytes) {
        const chunk = Buffer.from(bytes);
        this.#chunks.push(chunk);
        this.emit('data', this.#encoding ? chunk.toString(this.#encoding) : chunk);
        this.emit('end');
        this.#resolve(this.#chunks);
    }

    toArray() {
        return this.#done;
    }
}

class SpawnedChildProcess extends EventEmitter {
    #command;
    #args;
    #options;
    #input;
    #started = false;

    constructor(command, args, options) {
        super();
        this.#command = command;
        this.#args = args;
        this.#options = options;
        this.exitCode = null;
        this.signalCode = null;
        this.stdout = new BufferedReadable();
        this.stderr = new BufferedReadable();
        this.stdin = {
            end: (input = '') => {
                const bytes = input instanceof Uint8Array ? input : new TextEncoder().encode(String(input));
                this.#input = Array.from(bytes);
                this.#start();
            },
        };
        queueMicrotask(() => this.#start());
    }

    #start() {
        if (this.#started) return;
        this.#started = true;
        const selfHosted = this.#command === globalThis.__NODE_TEST_EXEC_PATH__;
        const args = selfHosted ? ['--node-compat-cli', ...this.#args] : this.#args;
        const command = new Deno.Command(this.#command, {
            args,
            cwd: this.#options.cwd,
            env: this.#options.env,
            stdin: this.#input,
        });
        command.output().then((output) => {
            this.exitCode = output.code;
            this.stdout.finish(output.stdout);
            this.stderr.finish(output.stderr);
            setTimeout(() => {
                this.emit('exit', output.code, null);
                this.emit('close', output.code, null);
            }, 0);
        }, (error) => {
            this.stdout.finish(new Uint8Array());
            this.stderr.finish(new Uint8Array());
            setTimeout(() => {
                this.emit('error', error);
                this.emit('close', -1, null);
            }, 0);
        });
    }
}

export function spawn(command, args = [], options = {}) {
    if (typeof command !== 'string') {
        throw new TypeError('The "command" argument must be of type string.');
    }
    if (!Array.isArray(args)) {
        options = args || {};
        args = [];
    }
    return new SpawnedChildProcess(command, args.map(String), options || {});
}

export function spawnSync(command, args = [], options = {}) {
    if (typeof command !== 'string') {
        throw new TypeError('The "command" argument must be of type string.');
    }
    if (!Array.isArray(args)) {
        options = args || {};
        args = [];
    }
    options ||= {};
    const selfHosted = command === globalThis.__NODE_TEST_EXEC_PATH__;
    const normalizedArgs = args.map(String);
    const commandArgs = selfHosted
        ? ['--node-compat-cli', ...normalizedArgs]
        : normalizedArgs;
    let stdin = null;
    if (options.input !== undefined && options.input !== null) {
        const bytes = options.input instanceof Uint8Array
            ? options.input
            : new TextEncoder().encode(String(options.input));
        stdin = Array.from(bytes);
    }
    const output = new Deno.Command(command, {
        args: commandArgs,
        cwd: options.cwd,
        env: options.env,
        stdin,
    }).outputSync();
    const encoding = options.encoding;
    const stdout = Buffer.from(output.stdout);
    const stderr = Buffer.from(output.stderr);
    const result = {
        pid: undefined,
        output: [null, stdout, stderr],
        stdout,
        stderr,
        status: output.code,
        signal: output.signal,
        error: undefined,
    };
    if (encoding && encoding !== 'buffer') {
        result.output = [null, stdout.toString(encoding), stderr.toString(encoding)];
        result.stdout = result.output[1];
        result.stderr = result.output[2];
    }
    return result;
}

export function fork(modulePath, args = [], options = {}) {
    if (typeof modulePath !== 'string') {
        throw new TypeError('The "modulePath" argument must be of type string.');
    }
    if (!Array.isArray(args)) {
        options = args || {};
        args = [];
    }
    return new ChildProcess(modulePath, args.map(String), options || {});
}

export const exec = (...args) => globalThis.child_process.exec(...args);

export default { ChildProcess, exec, fork, spawn, spawnSync };
export { ChildProcess };
