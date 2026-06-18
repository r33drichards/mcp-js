export { Buffer } from 'buffer';
export const process = { env: { NODE_ENV: 'production' }, platform: 'linux', browser: true, cwd: () => '/', nextTick: (f, ...a) => Promise.resolve().then(() => f(...a)) };
