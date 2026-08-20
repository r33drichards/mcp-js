// node:net — address helpers plus an inert Socket, sized for libraries
// (like @grpc/grpc-js) that import net for IP classification and socket
// *types*. Raw TCP is deliberately not provided: sandbox networking goes
// through the policy-gated fetch / WebSocket / node:http2 capabilities,
// where per-request policy and server-side header injection hold. A real
// net.connect would void both guarantees (see docs/node-http2-grpc-plan.md).

import { EventEmitter } from 'node:events';

const V4_SEG = '(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])';
const V4_RE = new RegExp(`^${V4_SEG}\\.${V4_SEG}\\.${V4_SEG}\\.${V4_SEG}$`);

export function isIPv4(input) {
    return typeof input === 'string' && V4_RE.test(input);
}

export function isIPv6(input) {
    if (typeof input !== 'string' || input.length === 0) return false;
    if (input.includes('.')) {
        // Mixed notation ::ffff:1.2.3.4
        const lastColon = input.lastIndexOf(':');
        if (lastColon === -1) return false;
        if (!isIPv4(input.slice(lastColon + 1))) return false;
        input = input.slice(0, lastColon + 1) + '0:0';
    }
    const parts = input.split('::');
    if (parts.length > 2) return false;
    const groups = (side) => (side === '' ? [] : side.split(':'));
    const head = groups(parts[0]);
    const tail = parts.length === 2 ? groups(parts[1]) : [];
    if (head.some((g) => !/^[0-9a-fA-F]{1,4}$/.test(g))) return false;
    if (tail.some((g) => !/^[0-9a-fA-F]{1,4}$/.test(g))) return false;
    const total = head.length + tail.length;
    if (parts.length === 2) return total < 8;
    return total === 8;
}

export function isIP(input) {
    if (isIPv4(input)) return 4;
    if (isIPv6(input)) return 6;
    return 0;
}

let defaultAutoSelectFamilyAttemptTimeout = 2500;

export function getDefaultAutoSelectFamilyAttemptTimeout() {
    return defaultAutoSelectFamilyAttemptTimeout;
}

export function setDefaultAutoSelectFamilyAttemptTimeout(value) {
    if (typeof value !== 'number') {
        const error = new TypeError(
            `The "value" argument must be of type number. Received type ${typeof value}`);
        error.code = 'ERR_INVALID_ARG_TYPE';
        throw error;
    }
    if (!Number.isInteger(value)) {
        const error = new RangeError(
            `The value of "value" is out of range. It must be an integer. Received ${value}`);
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    if (value < 1 || value > 0x7fffffff) {
        const error = new RangeError(
            `The value of "value" is out of range. It must be >= 1 && <= 2147483647. Received ${value}`);
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    defaultAutoSelectFamilyAttemptTimeout = Math.max(10, value);
}

/// Inert socket: satisfies "construct, configure, listen for events" call
/// patterns without any transport behind it. Anything that would actually
/// move bytes emits an error explaining the capability model.
export class Socket extends EventEmitter {
    constructor(_options) {
        super();
        this.connecting = false;
        this.destroyed = false;
        this.remoteAddress = undefined;
        this.remotePort = undefined;
    }

    connect() {
        const self = this;
        Promise.resolve().then(() => {
            self.emit('error', new Error(
                'net.Socket.connect is not supported in this runtime: use the ' +
                'policy-gated fetch / WebSocket / node:http2 capabilities instead'));
        });
        return this;
    }

    setNoDelay() { return this; }
    setKeepAlive() { return this; }
    setTimeout() { return this; }
    ref() { return this; }
    unref() { return this; }
    write() { return false; }
    end() { return this; }
    destroy() { this.destroyed = true; return this; }
}

export function connect() {
    return new Socket().connect();
}

export const createConnection = connect;

export function createServer() {
    throw new Error('net.createServer is not supported in this runtime');
}

export default {
    isIP,
    isIPv4,
    isIPv6,
    getDefaultAutoSelectFamilyAttemptTimeout,
    setDefaultAutoSelectFamilyAttemptTimeout,
    Socket,
    connect,
    createConnection,
    createServer,
};
