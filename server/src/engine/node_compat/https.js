// node:https — import-compatible stub, same posture as node:http: HTTPS
// in the sandbox is fetch() (or node:http2 for gRPC), and this module
// exists so libraries whose optional code paths import node:https (proxy
// tunnels, keep-alive agent setup) can load. Every operation throws.

import { Agent as HttpAgent, ClientRequest, IncomingMessage } from 'node:http';

export class Agent extends HttpAgent {}

export const globalAgent = new Agent();

export { ClientRequest, IncomingMessage };

export function request() {
    throw new Error(
        'https.request is not supported in this runtime: use fetch() ' +
        'or node:http2 instead');
}

export const get = request;

export function createServer() {
    throw new Error('https.createServer is not supported in this runtime');
}

export default {
    Agent,
    globalAgent,
    ClientRequest,
    IncomingMessage,
    request,
    get,
    createServer,
};
