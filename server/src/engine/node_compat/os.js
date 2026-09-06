// node:os — fixed values for the sandboxed runtime.
const os = {
    EOL: '\n',
    devNull: '/dev/null',
    platform: () => 'linux',
    type: () => 'Linux',
    release: () => '6.0.0',
    arch: () => 'x64',
    machine: () => 'x86_64',
    endianness: () => 'LE',
    hostname: () => 'mcp-v8',
    homedir: () => '/',
    tmpdir: () => '/tmp',
    cpus: () => [],
    totalmem: () => 0,
    freemem: () => 0,
    uptime: () => 0,
    loadavg: () => [0, 0, 0],
    availableParallelism: () => 1,
    networkInterfaces: () => ({}),
    userInfo: () => ({ uid: -1, gid: -1, username: 'sandbox', homedir: '/', shell: null }),
    version: () => 'mcp-v8',
    constants: { signals: {}, errno: {}, priority: {} },
};
export default os;
export const {
    EOL, devNull, platform, type, release, arch, machine, endianness,
    hostname, homedir, tmpdir, cpus, totalmem, freemem, uptime, loadavg,
    availableParallelism, networkInterfaces, userInfo, constants,
} = os;
export const version = os.version;
