// node:module — createRequire over the builtin registry. Every served
// node: module is imported eagerly so the require() a bundler emits
// (`const require = createRequire(import.meta.url)`) can hand back its
// CJS shape — the default export — synchronously. Only builtins resolve:
// file and package requires throw MODULE_NOT_FOUND, pointing at import.

import assert from 'node:assert';
import buffer from 'node:buffer';
import consoleModule from 'node:console';
import crypto from 'node:crypto';
import dns from 'node:dns';
import events from 'node:events';
import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import http from 'node:http';
import http2 from 'node:http2';
import https from 'node:https';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import querystring from 'node:querystring';
import stream from 'node:stream';
import streamWeb from 'node:stream/web';
import timers from 'node:timers';
import timersPromises from 'node:timers/promises';
import tls from 'node:tls';
import url from 'node:url';
import util from 'node:util';
import zlib from 'node:zlib';

const builtins = new Map([
    ['assert', assert],
    ['assert/strict', assert.strict],
    ['buffer', buffer],
    ['console', consoleModule],
    ['crypto', crypto],
    ['dns', dns],
    ['events', events],
    ['fs', fs],
    ['fs/promises', fsPromises],
    ['http', http],
    ['http2', http2],
    ['https', https],
    ['module', undefined], // replaced below; the Map must know the name
    ['net', net],
    ['os', os],
    ['path', path],
    ['process', process],
    ['querystring', querystring],
    ['stream', stream],
    ['stream/web', streamWeb],
    ['timers', timers],
    ['timers/promises', timersPromises],
    ['tls', tls],
    ['url', url],
    ['util', util],
    ['zlib', zlib],
]);

// Matches Node: subpath builtins are listed, alias names are not.
export const builtinModules = Object.freeze(
    [...builtins.keys()].filter((name) => name !== 'assert/strict'));

export function isBuiltin(name) {
    return builtins.has(String(name).replace(/^node:/, ''));
}

export function createRequire(_filename) {
    function require(id) {
        const name = String(id).replace(/^node:/, '');
        if (builtins.has(name)) return builtins.get(name);
        const err = new Error(
            "Cannot find module '" + id + "': only node: builtins resolve " +
            'through require() in this runtime; use import for everything else');
        err.code = 'MODULE_NOT_FOUND';
        throw err;
    }
    require.resolve = function resolve(id) {
        const name = String(id).replace(/^node:/, '');
        if (builtins.has(name)) return 'node:' + name;
        const err = new Error("Cannot find module '" + id + "'");
        err.code = 'MODULE_NOT_FOUND';
        throw err;
    };
    require.cache = Object.create(null);
    require.main = undefined;
    return require;
}

export function syncBuiltinESMExports() {
    // Builtins here are plain ESM with live bindings; nothing to sync.
}

const Module = {
    builtinModules,
    isBuiltin,
    createRequire,
    syncBuiltinESMExports,
};
builtins.set('module', Module);

export default Module;
