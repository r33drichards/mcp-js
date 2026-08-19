// node:module — createRequire over the builtin registry. Every served
// node: module is imported eagerly so the require() a bundler emits
// (`const require = createRequire(import.meta.url)`) can hand back its
// CJS shape — the default export — synchronously. Only builtins resolve:
// file and package requires throw MODULE_NOT_FOUND, pointing at import.

import assert from 'node:assert';
import buffer from 'node:buffer';
import childProcess from 'node:child_process';
import consoleModule from 'node:console';
import crypto from 'node:crypto';
import dns from 'node:dns';
import dnsPromises from 'node:dns/promises';
import events from 'node:events';
import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import http from 'node:http';
import http2 from 'node:http2';
import https from 'node:https';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import perfHooks from 'node:perf_hooks';
import process from 'node:process';
import querystring from 'node:querystring';
import stream from 'node:stream';
import streamWeb from 'node:stream/web';
import test from 'node:test';
import timers from 'node:timers';
import timersPromises from 'node:timers/promises';
import tls from 'node:tls';
import url from 'node:url';
import util from 'node:util';
import utilTypes from 'node:util/types';
import zlib from 'node:zlib';

const builtins = new Map([
    ['assert', assert],
    ['assert/strict', assert.strict],
    ['buffer', buffer],
    ['child_process', childProcess],
    ['console', consoleModule],
    ['crypto', crypto],
    ['dns', dns],
    ['dns/promises', dnsPromises],
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
    ['perf_hooks', perfHooks],
    ['process', process],
    ['querystring', querystring],
    ['stream', stream],
    ['stream/web', streamWeb],
    ['test', test],
    ['timers', timers],
    ['timers/promises', timersPromises],
    ['tls', tls],
    ['url', url],
    ['util', util],
    ['util/types', utilTypes],
    ['zlib', zlib],
]);

if (typeof __mcpV8InternalEsmResolve !== 'undefined') {
    builtins.set('internal/modules/esm/resolve', __mcpV8InternalEsmResolve);
}

// Matches Node: subpath builtins are listed, alias names are not.
export const builtinModules = Object.freeze(
    [...builtins.keys()].filter((name) =>
        name !== 'assert/strict' && !name.startsWith('internal/')));

export function isBuiltin(name) {
    const normalized = String(name).replace(/^node:/, '');
    return !normalized.startsWith('internal/') && builtins.has(normalized);
}

const virtualCommonJsModules = globalThis.__mcpV8VirtualCommonJsModules || null;
const virtualPackageJson = globalThis.__mcpV8VirtualPackageJson || null;
const virtualModuleCache = new Map();

function packageParts(specifier) {
    if (specifier.startsWith('.') || specifier.startsWith('/') || specifier.includes(':')) {
        return null;
    }
    const parts = specifier.split('/');
    const packageName = parts[0].startsWith('@')
        ? parts.splice(0, 2).join('/')
        : parts.shift();
    return { packageName, subpath: parts.join('/') };
}

const PACKAGE_TARGET_MISSING = Symbol('missing');
const PACKAGE_TARGET_INVALID = Symbol('invalid');

function packageTarget(value, match, conditions) {
    if (typeof value === 'string') {
        if (!value.startsWith('./')) return PACKAGE_TARGET_INVALID;
        const resolved = value.replaceAll('*', () => match).replace(/\/+/g, '/');
        if (resolved.slice(2).split('/').some((segment) =>
            decodeURIComponent(segment).toLowerCase() === 'node_modules')) {
            return PACKAGE_TARGET_INVALID;
        }
        return resolved;
    }
    if (value === null) return PACKAGE_TARGET_MISSING;
    if (Array.isArray(value)) {
        let invalid = false;
        for (const target of value) {
            const resolved = packageTarget(target, match, conditions);
            if (typeof resolved === 'string') return resolved;
            if (resolved === PACKAGE_TARGET_INVALID) invalid = true;
        }
        return invalid ? PACKAGE_TARGET_INVALID : PACKAGE_TARGET_MISSING;
    }
    if (value && typeof value === 'object') {
        for (const [condition, target] of Object.entries(value)) {
            if (condition === 'default' || conditions.includes(condition)) {
                const resolved = packageTarget(target, match, conditions);
                if (resolved !== PACKAGE_TARGET_MISSING) return resolved;
            }
        }
        return PACKAGE_TARGET_MISSING;
    }
    return PACKAGE_TARGET_INVALID;
}

function exportsTarget(exports, packageSubpath, conditions) {
    if (!exports || typeof exports !== 'object' || Array.isArray(exports)) {
        return packageSubpath === '.'
            ? packageTarget(exports, '', conditions)
            : PACKAGE_TARGET_MISSING;
    }
    const keys = Object.keys(exports);
    if (keys.some((key) => /^(0|[1-9]\d*)$/.test(key))) {
        const err = new Error('Invalid package config: "exports" cannot contain numeric property keys');
        err.code = 'ERR_INVALID_PACKAGE_CONFIG';
        throw err;
    }
    const subpathKeys = keys.filter((key) => key.startsWith('.'));
    if (subpathKeys.length > 0 && subpathKeys.length !== keys.length) {
        const err = new Error(
            'Invalid package config: "exports" cannot contain some keys starting with \'.\' ' +
            'and some not. The exports object must either be an object of package subpath keys ' +
            'or an object of main entry condition name keys only.');
        err.code = 'ERR_INVALID_PACKAGE_CONFIG';
        throw err;
    }
    if (subpathKeys.length === 0) {
        return packageSubpath === '.'
            ? packageTarget(exports, '', conditions)
            : PACKAGE_TARGET_MISSING;
    }
    if (Object.prototype.hasOwnProperty.call(exports, packageSubpath)) {
        return packageTarget(exports[packageSubpath], '', conditions);
    }
    const patterns = keys
        .map((key) => {
            const wildcard = key.indexOf('*');
            if (wildcard < 0) return null;
            const prefix = key.slice(0, wildcard);
            const suffix = key.slice(wildcard + 1);
            if (packageSubpath.length <= prefix.length + suffix.length ||
                !packageSubpath.startsWith(prefix) || !packageSubpath.endsWith(suffix)) return null;
            return {
                prefix: prefix.length,
                suffix: suffix.length,
                match: packageSubpath.slice(prefix.length, packageSubpath.length - suffix.length),
                target: exports[key],
            };
        })
        .filter(Boolean)
        .sort((a, b) => b.prefix - a.prefix || b.suffix - a.suffix);
    if (patterns.length === 0) return PACKAGE_TARGET_MISSING;
    return packageTarget(patterns[0].target, patterns[0].match, conditions);
}

function virtualFileUrl(filename) {
    const value = String(filename);
    if (value.startsWith('file:')) return new URL(value);
    return new URL('file://' + (value.startsWith('/') ? value : '/' + value));
}

function resolveVirtualFile(url) {
    const candidates = [url.href];
    if (!/\.[^/]+$/.test(url.pathname)) {
        candidates.push(url.href + '.js', url.href + '.cjs', url.href + '.json');
        candidates.push(new URL('./index.js', url.href.endsWith('/') ? url : url.href + '/').href);
    }
    return candidates.find((candidate) =>
        virtualCommonJsModules && Object.prototype.hasOwnProperty.call(virtualCommonJsModules, candidate));
}

function resolvePackageSource(packageUrl, source, parts, conditions) {
    const packageData = JSON.parse(source);
    const packageSubpath = parts.subpath ? './' + parts.subpath : '.';
    const hasExports = packageData.exports !== undefined;
    const target = !hasExports
        ? (parts.subpath ? './' + parts.subpath : packageData.main || './index.js')
        : exportsTarget(packageData.exports, packageSubpath, conditions);
    if (target === PACKAGE_TARGET_INVALID) {
        const err = new Error(
            `Invalid "exports" target for '${packageSubpath}'; targets must start with './'`);
        err.code = 'ERR_INVALID_PACKAGE_TARGET';
        throw err;
    }
    if (target === PACKAGE_TARGET_MISSING) {
        const message = packageSubpath === '.'
            ? 'No "exports" main defined in ' + packageUrl.href
            : `Package subpath '${packageSubpath}' is not defined by exports`;
        const err = new Error(message);
        err.code = 'ERR_PACKAGE_PATH_NOT_EXPORTED';
        throw err;
    }
    const targetUrl = new URL(target, packageUrl);
    const resolved = hasExports
        ? (virtualCommonJsModules &&
            Object.prototype.hasOwnProperty.call(virtualCommonJsModules, targetUrl.href)
            ? targetUrl.href
            : null)
        : resolveVirtualFile(targetUrl);
    if (resolved) return resolved;
    const err = new Error(`Cannot find module '${decodeURIComponent(targetUrl.pathname)}'`);
    err.code = 'MODULE_NOT_FOUND';
    throw err;
}

function resolveVirtual(id, filename, conditions = ['require', 'node', 'default']) {
    if (!virtualCommonJsModules || !virtualPackageJson) return null;
    const request = String(id);
    if (request.startsWith('./') || request.startsWith('../') || request.startsWith('/')) {
        return resolveVirtualFile(new URL(request, virtualFileUrl(filename)));
    }
    const parts = packageParts(request);
    if (!parts) return null;
    if (parts.subpath) {
        const invalid = parts.subpath.split('/').some((segment) => {
            let decoded;
            try {
                decoded = decodeURIComponent(segment).toLowerCase();
            } catch {
                return true;
            }
            return decoded === '.' || decoded === '..' || decoded === 'node_modules' ||
                decoded.includes('/') || decoded.includes('\\');
        });
        if (invalid) {
            const packageSubpath = './' + parts.subpath;
            const err = new Error(
                `Invalid module '${request}' is not a valid match in pattern '${packageSubpath}'`);
            err.code = 'ERR_INVALID_MODULE_SPECIFIER';
            throw err;
        }
    }
    let directory = new URL('.', virtualFileUrl(filename));
    while (true) {
        const selfPackageUrl = new URL('package.json', directory);
        const selfSource = virtualPackageJson[selfPackageUrl.href];
        if (selfSource !== undefined && JSON.parse(selfSource).name === parts.packageName) {
            return resolvePackageSource(selfPackageUrl, selfSource, parts, conditions);
        }
        const insideNodeModules = directory.pathname.endsWith('/node_modules/');
        const packageBase = new URL(`node_modules/${parts.packageName}/`, directory);
        const packageUrl = new URL('package.json', packageBase);
        const source = insideNodeModules ? undefined : virtualPackageJson[packageUrl.href];
        if (source !== undefined) return resolvePackageSource(packageUrl, source, parts, conditions);
        const legacyFile = insideNodeModules
            ? null
            : resolveVirtualFile(new URL(`node_modules/${request}`, directory));
        if (legacyFile) return legacyFile;
        const legacyPackage = insideNodeModules ? null : resolveVirtualFile(new URL(
            parts.subpath || './index.js', packageBase));
        if (legacyPackage) return legacyPackage;
        const parent = new URL('../', directory);
        if (parent.href === directory.href) return null;
        directory = parent;
    }
}

function virtualNodeModulePaths(filename) {
    const paths = [];
    let directory = new URL('.', virtualFileUrl(filename));
    while (true) {
        if (!directory.pathname.endsWith('/node_modules/')) {
            paths.push(decodeURIComponent(new URL('node_modules/', directory).pathname).replace(/\/$/, ''));
        }
        const parent = new URL('../', directory);
        if (parent.href === directory.href) return paths;
        directory = parent;
    }
}

function importVirtualModule(id, filename) {
    const name = String(id).replace(/^node:/, '');
    if (builtins.has(name)) {
        const value = builtins.get(name);
        return Promise.resolve({ ...value, default: value, 'module.exports': value });
    }
    try {
        const resolved = resolveVirtual(id, filename, ['import', 'node', 'default']);
        if (!resolved) return Promise.reject(new Error(`Cannot find module '${id}'`));
        const value = loadVirtualModule(resolved);
        return Promise.resolve({ ...value, default: value, 'module.exports': value });
    } catch (error) {
        return Promise.reject(error);
    }
}

function loadVirtualModule(specifier) {
    if (virtualModuleCache.has(specifier)) return virtualModuleCache.get(specifier).exports;
    const source = virtualCommonJsModules[specifier];
    if (source === undefined) return undefined;
    if (specifier.endsWith('.json')) return JSON.parse(source);
    const filename = decodeURIComponent(new URL(specifier).pathname);
    const module = {
        exports: {},
        filename,
        id: filename,
        loaded: false,
        paths: virtualNodeModulePaths(specifier),
    };
    virtualModuleCache.set(specifier, module);
    const dirname = filename.slice(0, filename.lastIndexOf('/')) || '/';
    const localRequire = createRequire(specifier);
    const compiledSource = source.replace(/\bimport\s*\(/g, '__mcpV8Import(');
    Function(
        'exports', 'require', 'module', '__filename', '__dirname', '__mcpV8Import', compiledSource,
    ).call(
        module.exports,
        module.exports,
        localRequire,
        module,
        filename,
        dirname,
        (id) => importVirtualModule(id, specifier),
    );
    module.loaded = true;
    return module.exports;
}

export function createRequire(_filename) {
    function require(id) {
        const name = String(id).replace(/^node:/, '');
        if (builtins.has(name)) return builtins.get(name);
        const resolved = resolveVirtual(id, _filename);
        if (resolved) return loadVirtualModule(resolved);
        const err = new Error("Cannot find module '" + id + "'");
        err.code = 'MODULE_NOT_FOUND';
        throw err;
    }
    require.resolve = function resolve(id) {
        const name = String(id).replace(/^node:/, '');
        if (builtins.has(name)) return 'node:' + name;
        const resolved = resolveVirtual(id, _filename);
        if (resolved) return decodeURIComponent(new URL(resolved).pathname);
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
