// node:http — import-compatible stub. HTTP/1 in the sandbox is fetch();
// this module exists so libraries whose optional code paths import
// node:http (e.g. CONNECT proxy support in gRPC stacks, unused when no
// proxy env vars are set) can load. Every operation throws.

import { EventEmitter } from 'node:events';

export class Agent {
    constructor(options) {
        this.options = options || {};
    }
    destroy() {}
}

export const globalAgent = new Agent();

export class ClientRequest extends EventEmitter {}
export class IncomingMessage extends EventEmitter {}

export function request() {
    throw new Error(
        'http.request is not supported in this runtime: use fetch() (HTTP/1) ' +
        'or node:http2 instead');
}

export const get = request;

export function createServer() {
    throw new Error('http.createServer is not supported in this runtime');
}

export const STATUS_CODES = Object.freeze({});

export default {
    Agent,
    globalAgent,
    ClientRequest,
    IncomingMessage,
    request,
    get,
    createServer,
    STATUS_CODES,
};
