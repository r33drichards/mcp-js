// node:timers — the runtime timer globals under Node's module name. The
// globals already return Node-style Timeout handles (ref/unref/refresh,
// numeric valueOf), so this is a re-export plus the promises namespace.
// Legacy enroll/unenroll/active are not provided.

import promises from 'node:timers/promises';

const g = globalThis;

export const setTimeout = g.setTimeout;
export const clearTimeout = g.clearTimeout;
export const setInterval = g.setInterval;
export const clearInterval = g.clearInterval;
export const setImmediate = g.setImmediate;
export const clearImmediate = g.clearImmediate;
export { promises };

export default {
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    setImmediate,
    clearImmediate,
    promises,
};
