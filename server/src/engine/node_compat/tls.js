// node:tls — the option-plumbing subset gRPC-style libraries touch on the
// client path. TLS itself terminates host-side inside the policy-gated
// transports (fetch / WebSocket / node:http2), so createSecureContext just
// carries options through and checkServerIdentity accepts (real verification
// happened in rustls before any bytes reached the isolate). Raw tls.connect
// is not provided, same reasoning as node:net.

import { Socket } from 'node:net';

export function createSecureContext(options) {
    // Opaque token; consumers pass it back into http2.connect options where
    // the host-side transport ignores it (the host owns trust roots).
    return { context: options || {} };
}

export function checkServerIdentity(_hostname, _cert) {
    // Certificate verification is performed host-side by rustls during the
    // transport handshake; there is no peer certificate to re-verify here.
    return undefined;
}

export const rootCertificates = Object.freeze([]);

export class TLSSocket extends Socket {
    getPeerCertificate() { return {}; }
    getCipher() { return undefined; }
    alpnProtocol = false;
}

export function connect() {
    return new TLSSocket().connect();
}

export default {
    createSecureContext,
    checkServerIdentity,
    rootCertificates,
    TLSSocket,
    connect,
    CLIENT_RENEG_LIMIT: 3,
    CLIENT_RENEG_WINDOW: 600,
};
