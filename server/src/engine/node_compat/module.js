// node:module — createRequire over the builtin registry. Every served
// node: module is imported eagerly so the require() a bundler emits
// (`const require = createRequire(import.meta.url)`) can hand back its
// CJS shape — the default export — synchronously. Only builtins resolve:
// file and package requires throw MODULE_NOT_FOUND, pointing at import.

import assert from 'node:assert';
import asyncHooks from 'node:async_hooks';
import buffer from 'node:buffer';
import childProcess from 'node:child_process';
import consoleModule from 'node:console';
import crypto from 'node:crypto';
import diagnosticsChannel from 'node:diagnostics_channel';
import cluster from 'node:cluster';
import dgram from 'node:dgram';
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
import tty from 'node:tty';
import url from 'node:url';
import util from 'node:util';
import utilTypes from 'node:util/types';
import workerThreads from 'node:worker_threads';
import zlib from 'node:zlib';

const builtins = new Map([
    // Legacy require-able http internals, aliased onto the http module.
    ['_http_agent', { Agent: http.Agent, globalAgent: http.globalAgent }],
    ['_http_client', { ClientRequest: http.ClientRequest }],
    ['_http_common', {
        methods: http.METHODS,
        chunkExpression: /(?:^|\W)chunked(?:$|\W)/i,
        _checkIsHttpToken: http._checkIsHttpToken,
        _checkInvalidHeaderChar: http._checkInvalidHeaderChar,
    }],
    ['_http_server', {
        STATUS_CODES: http.STATUS_CODES,
        Server: http.Server,
        ServerResponse: http.ServerResponse,
        kConnectionsCheckingInterval: http.kConnectionsCheckingInterval,
    }],
    ['assert', assert],
    ['assert/strict', assert.strict],
    ['async_hooks', asyncHooks],
    ['buffer', buffer],
    ['child_process', childProcess],
    ['console', consoleModule],
    ['crypto', crypto],
    ['diagnostics_channel', diagnosticsChannel],
    ['cluster', cluster],
    ['dgram', dgram],
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
    ['tty', tty],
    ['url', url],
    ['util', util],
    ['util/types', utilTypes],
    ['worker_threads', workerThreads],
    ['zlib', zlib],
]);

if (typeof __mcpV8InternalEsmResolve !== 'undefined') {
    builtins.set('internal/modules/esm/resolve', __mcpV8InternalEsmResolve);
}

// Matches Node: subpath builtins are listed, alias names are not. The
// _http_* legacy aliases stay require-able but unlisted — the registry
// contract (builtin_modules_matches_registry) pins this list to the
// modules the loader actually serves.
export const builtinModules = Object.freeze(
    [...builtins.keys()].filter((name) =>
        name !== 'assert/strict' && !name.startsWith('internal/')
        && !name.startsWith('_http_')));

export function isBuiltin(name) {
    const normalized = String(name).replace(/^node:/, '');
    return !normalized.startsWith('internal/') && builtins.has(normalized);
}

const virtualCommonJsModules = globalThis.__mcpV8VirtualCommonJsModules || null;
const virtualPackageJson = globalThis.__mcpV8VirtualPackageJson || null;
const virtualPackageMap = globalThis.__mcpV8PackageMap || null;
const virtualModuleCache = new Map();
const requireCache = Object.create(null);
globalThis.__mcpV8IsVirtualCommonJsModule = (specifier) => {
    if (!virtualCommonJsModules) return false;
    const url = String(specifier);
    if (Object.prototype.hasOwnProperty.call(virtualCommonJsModules, url)) return true;
    try {
        const normalized = new URL(url);
        normalized.search = '';
        normalized.hash = '';
        return Object.prototype.hasOwnProperty.call(virtualCommonJsModules, normalized.href);
    } catch {
        return false;
    }
};
const emittedPackageWarnings = globalThis.__mcpV8EmittedPackageWarnings ??= new Set();
let packageDeprecationSerial = 0;
const ESM_IMPORT_PREFIX = 'mcp-v8:esm-import:';
const ORIGINAL_ESM_PREFIX = '/*mcp-v8-original-esm:';

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
const PACKAGE_TARGET_NULL = Symbol('null');
const PACKAGE_TARGET_INVALID = Symbol('invalid');
const PACKAGE_TARGET_INVALID_SPECIFIER = Symbol('invalid-specifier');

function packageTarget(value, match, conditions) {
    if (typeof value === 'string') {
        if (!value.startsWith('./')) return PACKAGE_TARGET_INVALID;
        const resolved = value.replaceAll('*', () => match);
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
        const target = packageSubpath === '.'
            ? packageTarget(exports, '', conditions)
            : PACKAGE_TARGET_MISSING;
        return typeof target === 'string'
            ? { target, patternKey: null, patternMatch: null }
            : target;
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
        const target = packageSubpath === '.'
            ? packageTarget(exports, '', conditions)
            : PACKAGE_TARGET_MISSING;
        return typeof target === 'string'
            ? { target, patternKey: null, patternMatch: null }
            : target;
    }
    if (Object.prototype.hasOwnProperty.call(exports, packageSubpath)) {
        const target = packageTarget(exports[packageSubpath], '', conditions);
        return typeof target === 'string'
            ? { target, patternKey: null, patternMatch: null }
            : target;
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
                key,
                prefix: prefix.length,
                suffix: suffix.length,
                match: packageSubpath.slice(prefix.length, packageSubpath.length - suffix.length),
                target: exports[key],
            };
        })
        .filter(Boolean)
        .sort((a, b) => b.prefix - a.prefix || b.suffix - a.suffix);
    if (patterns.length === 0) return PACKAGE_TARGET_MISSING;
    const pattern = patterns[0];
    const target = packageTarget(pattern.target, pattern.match, conditions);
    return typeof target === 'string'
        ? { target, patternKey: pattern.key, patternMatch: pattern.match }
        : target;
}

function emitPackageExportWarning(resolution, packageSubpath, packageUrl, referrer, isImport) {
    const pattern = resolution.patternKey === null
        ? ''
        : ` matched to "${resolution.patternKey}"`;
    const importedFrom = isImport
        ? ` imported from ${decodeURIComponent(virtualFileUrl(referrer).pathname)}`
        : '';
    const location = ` in the "exports" field module resolution of the package at ${decodeURIComponent(packageUrl.pathname)}${importedFrom}.`;
    if (packageSubpath.endsWith('/') && resolution.patternKey !== null) {
        packageDeprecationSerial += 1;
        const warningKey = `DEP0155:${decodeURIComponent(packageUrl.pathname)}:${packageSubpath}`;
        if (emittedPackageWarnings.has(warningKey)) return;
        emittedPackageWarnings.add(warningKey);
        process.emitWarning(
            `Use of deprecated trailing slash pattern mapping "${packageSubpath}"${pattern}${location}`,
            'DeprecationWarning',
            'DEP0155',
        );
        return;
    }
    const kind = resolution.target.includes('//')
        ? 'double slash'
        : resolution.patternMatch !== null &&
            (resolution.patternMatch.startsWith('/') || resolution.patternMatch.endsWith('/'))
            ? 'leading or trailing slash matching'
            : null;
    if (kind === null) return;
    packageDeprecationSerial += 1;
    process.emitWarning(
        `Use of deprecated ${kind} resolving "${resolution.target}" for module request "${packageSubpath}"${pattern}${location}`,
        'DeprecationWarning',
        'DEP0166',
    );
}

function emitPackageMainWarning(packageData, packageUrl, referrer, resolved) {
    const main = typeof packageData.main === 'string' && packageData.main.length > 0
        ? packageData.main
        : null;
    if (main !== null && /\.[^/]+$/.test(main)) return;
    const warningKey = `DEP0151:${packageUrl.href}:${main ?? ''}`;
    if (emittedPackageWarnings.has(warningKey)) return;
    emittedPackageWarnings.add(warningKey);
    const packageRoot = decodeURIComponent(new URL('./', packageUrl).pathname);
    const importedFrom = decodeURIComponent(virtualFileUrl(referrer).pathname);
    const message = main === null
        ? `No "main" or "exports" field defined in the package.json for ${packageRoot} resolving the main entry point "index.js", imported from ${importedFrom}.\nDefault "index" lookups for the main are deprecated for ES modules.`
        : `Package ${packageRoot} has a "main" field set to ${JSON.stringify(main)}, excluding the full filename and extension to the resolved file at ${JSON.stringify(new URL(resolved).pathname.split('/').pop())}, imported from ${importedFrom}.\n Automatic extension resolution of the "main" field is deprecated for ES modules.`;
    process.emitWarning(message, 'DeprecationWarning', 'DEP0151');
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

function resolvePackageSource(packageUrl, source, parts, conditions, referrer) {
    const packageData = JSON.parse(source);
    const packageSubpath = parts.subpath ? './' + parts.subpath : '.';
    const hasExports = packageData.exports !== undefined;
    const resolution = !hasExports
        ? (parts.subpath ? './' + parts.subpath : packageData.main || './index.js')
        : exportsTarget(packageData.exports, packageSubpath, conditions);
    if (resolution === PACKAGE_TARGET_INVALID) {
        const err = new Error(
            `Invalid "exports" target for '${packageSubpath}'; targets must start with './'`);
        err.code = 'ERR_INVALID_PACKAGE_TARGET';
        throw err;
    }
    if (resolution === PACKAGE_TARGET_MISSING) {
        const message = packageSubpath === '.'
            ? 'No "exports" main defined in ' + packageUrl.href
            : `Package subpath '${packageSubpath}' is not defined by exports`;
        const err = new Error(message);
        err.code = 'ERR_PACKAGE_PATH_NOT_EXPORTED';
        throw err;
    }
    const target = hasExports ? resolution.target : resolution;
    if (hasExports) {
        emitPackageExportWarning(
            resolution,
            packageSubpath,
            packageUrl,
            referrer,
            conditions.includes('import'),
        );
    }
    const normalizedTarget = hasExports
        ? './' + target.slice(2).replace(/^\/+/, '').replace(/\/+/g, '/')
        : target;
    const targetUrl = new URL(normalizedTarget, packageUrl);
    const resolved = hasExports
        ? (virtualCommonJsModules &&
            Object.prototype.hasOwnProperty.call(virtualCommonJsModules, targetUrl.href)
            ? targetUrl.href
            : null)
        : resolveVirtualFile(targetUrl);
    if (resolved) {
        if (!hasExports && conditions.includes('import') && parts.subpath === '') {
            emitPackageMainWarning(packageData, packageUrl, referrer, resolved);
        }
        return resolved;
    }
    const err = new Error(`Cannot find module '${decodeURIComponent(targetUrl.pathname)}'`);
    err.code = 'MODULE_NOT_FOUND';
    throw err;
}

function packageImportsTarget(value, match, conditions) {
    if (typeof value === 'string') {
        const target = value.replaceAll('*', () => match);
        if (target.startsWith('./')) {
            try {
                for (const segment of target.slice(2).split('/')) {
                    const decoded = decodeURIComponent(segment).toLowerCase();
                    if (decoded.includes('/') || decoded.includes('\\')) {
                        return PACKAGE_TARGET_INVALID_SPECIFIER;
                    }
                    if (decoded === '.' || decoded === '..' || decoded === 'node_modules') {
                        return PACKAGE_TARGET_INVALID;
                    }
                }
            } catch {
                return PACKAGE_TARGET_INVALID_SPECIFIER;
            }
            return { target, external: false };
        }
        if (target.startsWith('../') || target.startsWith('/') || target.includes(':')) {
            return PACKAGE_TARGET_INVALID;
        }
        return { target, external: true };
    }
    if (value === null) return PACKAGE_TARGET_NULL;
    if (Array.isArray(value)) {
        let invalid = false;
        let nullTarget = false;
        for (const target of value) {
            const resolved = packageImportsTarget(target, match, conditions);
            if (resolved && typeof resolved === 'object') return resolved;
            if (resolved === PACKAGE_TARGET_INVALID_SPECIFIER) return resolved;
            if (resolved === PACKAGE_TARGET_INVALID) invalid = true;
            if (resolved === PACKAGE_TARGET_NULL) nullTarget = true;
        }
        if (nullTarget) return PACKAGE_TARGET_NULL;
        return invalid ? PACKAGE_TARGET_INVALID : PACKAGE_TARGET_MISSING;
    }
    if (value && typeof value === 'object') {
        for (const [condition, target] of Object.entries(value)) {
            if (condition === 'default' || conditions.includes(condition)) {
                return packageImportsTarget(target, match, conditions);
            }
        }
        return PACKAGE_TARGET_MISSING;
    }
    return PACKAGE_TARGET_INVALID;
}

function importsTarget(imports, request, conditions) {
    if (!imports || typeof imports !== 'object' || Array.isArray(imports)) {
        return PACKAGE_TARGET_MISSING;
    }
    if (Object.prototype.hasOwnProperty.call(imports, request)) {
        return packageImportsTarget(imports[request], '', conditions);
    }
    const patterns = Object.keys(imports)
        .map((key) => {
            const wildcard = key.indexOf('*');
            if (wildcard < 0) return null;
            const prefix = key.slice(0, wildcard);
            const suffix = key.slice(wildcard + 1);
            if (request.length <= prefix.length + suffix.length ||
                !request.startsWith(prefix) || !request.endsWith(suffix)) return null;
            return {
                prefix: prefix.length,
                suffix: suffix.length,
                match: request.slice(prefix.length, request.length - suffix.length),
                target: imports[key],
            };
        })
        .filter(Boolean)
        .sort((a, b) => b.prefix - a.prefix || b.suffix - a.suffix);
    if (patterns.length === 0) return PACKAGE_TARGET_MISSING;
    const pattern = patterns[0];
    return packageImportsTarget(pattern.target, pattern.match, conditions);
}

function invalidPackageImportSpecifier(request, reason) {
    const error = new Error(`Invalid module '${request}' ${reason}`);
    error.code = 'ERR_INVALID_MODULE_SPECIFIER';
    return error;
}

function emitPackageImportWarning(request, resolution, filename) {
    const testFlags = globalThis.__NODE_TEST_FLAGS__;
    if (!process.execArgv.includes('--pending-deprecation') &&
        (!Array.isArray(testFlags) || !testFlags.includes('--pending-deprecation'))) return;
    if (!request.includes('//') && !resolution.target.includes('//')) return;
    process.emitWarning(
        `Use of deprecated double slash resolving "${resolution.target}" for module request "${request}" imported from ${filename}.`,
        'DeprecationWarning',
        'DEP0166',
    );
}

function resolvePackageImport(request, filename, conditions) {
    if (request === '#') {
        throw invalidPackageImportSpecifier(request, 'is not a valid internal imports specifier name');
    }
    try {
        let invalidMatch = false;
        if (request.split('/').some((segment) => {
            const decoded = decodeURIComponent(segment);
            if (decoded === '.' || decoded === '..' || decoded.toLowerCase() === 'node_modules') {
                invalidMatch = true;
            }
            return decoded.includes('/') || decoded.includes('\\');
        })) {
            throw invalidPackageImportSpecifier(request, 'must not include encoded "/" or "\\"');
        }
        if (invalidMatch) {
            throw invalidPackageImportSpecifier(request, 'request is not a valid match in pattern');
        }
    } catch (error) {
        if (error?.code === 'ERR_INVALID_MODULE_SPECIFIER') throw error;
        throw invalidPackageImportSpecifier(request, 'is not a valid internal imports specifier name');
    }

    let directory = new URL('.', virtualFileUrl(filename));
    while (true) {
        const packageUrl = new URL('package.json', directory);
        const source = virtualPackageJson[packageUrl.href];
        if (source !== undefined) {
            const packageData = JSON.parse(source);
            const resolution = importsTarget(packageData.imports, request, conditions);
            if (resolution === PACKAGE_TARGET_INVALID_SPECIFIER) {
                throw invalidPackageImportSpecifier(request, 'must not include encoded "/" or "\\"');
            }
            if (resolution === PACKAGE_TARGET_INVALID) {
                const error = new Error(`Invalid "imports" target for '${request}'`);
                error.code = 'ERR_INVALID_PACKAGE_TARGET';
                throw error;
            }
            if (resolution === PACKAGE_TARGET_MISSING || resolution === PACKAGE_TARGET_NULL) {
                const error = new Error(`Package import specifier "${request}" is not defined in ${packageUrl.href}`);
                error.code = 'ERR_PACKAGE_IMPORT_NOT_DEFINED';
                throw error;
            }
            emitPackageImportWarning(request, resolution, filename);
            if (resolution.external) {
                const resolved = resolveVirtual(resolution.target, packageUrl.href, conditions);
                if (resolved) return resolved;
                const error = new Error(`Cannot find module '${resolution.target}'`);
                error.code = 'MODULE_NOT_FOUND';
                throw error;
            }
            const targetUrl = new URL(resolution.target, packageUrl);
            if (!targetUrl.pathname.startsWith(directory.pathname)) {
                const error = new Error(`Invalid "imports" target for '${request}'`);
                error.code = 'ERR_INVALID_PACKAGE_TARGET';
                throw error;
            }
            const resolved = resolveVirtualFile(targetUrl);
            if (resolved) return resolved;
            const error = new Error(`Cannot find module '${decodeURIComponent(targetUrl.pathname)}'`);
            error.code = 'MODULE_NOT_FOUND';
            throw error;
        }
        const parent = new URL('../', directory);
        if (parent.href === directory.href) return null;
        directory = parent;
    }
}

function packageUrlPath(value) {
    return String(value).replaceAll('#', '%23').replaceAll('?', '%3F');
}

function resolvePackageMap(request, filename, conditions) {
    if (!virtualPackageMap) return null;
    const parts = packageParts(request);
    if (!parts) return null;
    const referrer = virtualFileUrl(filename).href;
    const owner = Object.values(virtualPackageMap.packages)
        .filter((entry) => referrer.startsWith(entry.url))
        .sort((left, right) => right.url.length - left.url.length)[0];
    if (!owner) {
        const error = new Error(`ERR_PACKAGE_MAP_EXTERNAL_FILE: File outside package map scope: ${referrer}`);
        error.code = 'ERR_PACKAGE_MAP_EXTERNAL_FILE';
        throw error;
    }
    const targetKey = owner.dependencies[parts.packageName];
    if (targetKey === undefined) {
        const error = new Error(`Cannot find module '${request}'`);
        error.code = 'MODULE_NOT_FOUND';
        throw error;
    }
    if (!Object.prototype.hasOwnProperty.call(virtualPackageMap.packages, targetKey)) {
        const error = new Error(`ERR_PACKAGE_MAP_KEY_NOT_FOUND: Package map key '${targetKey}' was not found`);
        error.code = 'ERR_PACKAGE_MAP_KEY_NOT_FOUND';
        throw error;
    }
    const target = virtualPackageMap.packages[targetKey];
    if (!target) {
        const error = new Error(`ERR_PACKAGE_MAP_KEY_NOT_FOUND: Package map key '${targetKey}' was not found`);
        error.code = 'ERR_PACKAGE_MAP_KEY_NOT_FOUND';
        throw error;
    }
    const packageUrl = new URL('package.json', target.url);
    const source = virtualPackageJson[packageUrl.href];
    if (source !== undefined) {
        return resolvePackageSource(packageUrl, source, parts, conditions, filename);
    }
    const requestPath = parts.subpath || 'index';
    const targetUrl = new URL(requestPath, target.url);
    if (!targetUrl.href.startsWith(target.url)) {
        const error = new Error(`Invalid module '${request}'`);
        error.code = 'ERR_INVALID_MODULE_SPECIFIER';
        throw error;
    }
    const resolved = resolveVirtualFile(targetUrl);
    if (resolved) return resolved;
    const error = new Error(`Cannot find module '${request}'`);
    error.code = 'MODULE_NOT_FOUND';
    throw error;
}

function resolveVirtual(id, filename, conditions = ['require', 'node', 'default']) {
    if (!virtualCommonJsModules || !virtualPackageJson) return null;
    const request = String(id);
    if (request.startsWith('#')) {
        const imported = resolvePackageImport(request, filename, conditions);
        if (imported) return imported;
    }
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
    const mapped = resolvePackageMap(request, filename, conditions);
    if (mapped) return mapped;
    let directory = new URL('.', virtualFileUrl(filename));
    while (true) {
        const selfPackageUrl = new URL('package.json', directory);
        const selfSource = virtualPackageJson[selfPackageUrl.href];
        if (selfSource !== undefined && JSON.parse(selfSource).name === parts.packageName) {
            return resolvePackageSource(selfPackageUrl, selfSource, parts, conditions, filename);
        }
        const insideNodeModules = directory.pathname.endsWith('/node_modules/');
        const packageBase = new URL(`node_modules/${packageUrlPath(parts.packageName)}/`, directory);
        const packageUrl = new URL('package.json', packageBase);
        const source = insideNodeModules ? undefined : virtualPackageJson[packageUrl.href];
        if (source !== undefined) {
            return resolvePackageSource(packageUrl, source, parts, conditions, filename);
        }
        const legacyFile = insideNodeModules
            ? null
            : resolveVirtualFile(new URL(`node_modules/${packageUrlPath(request)}`, directory));
        if (legacyFile) return legacyFile;
        const legacyPackage = insideNodeModules ? null : resolveVirtualFile(new URL(
            parts.subpath || './index.js', packageBase));
        if (legacyPackage) return legacyPackage;
        const parent = new URL('../', directory);
        if (parent.href === directory.href) return null;
        directory = parent;
    }
}

function resolveImportSpecifier(request, filename) {
    const name = request.replace(/^node:/, '');
    if (builtins.has(name)) return null;
    try {
        return resolveVirtual(request, filename, ['import', 'node', 'default']);
    } catch {
        return null;
    }
}

globalThis.__mcpV8ResolveImportSpecifier = resolveImportSpecifier;

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

function commonJsNamespace(value) {
    const named = value !== null && (typeof value === 'object' || typeof value === 'function')
        ? { ...value }
        : {};
    return { ...named, default: value, 'module.exports': value };
}

function importVirtualModule(id, filename) {
    const name = String(id).replace(/^node:/, '');
    if (builtins.has(name)) {
        const value = builtins.get(name);
        return Promise.resolve(commonJsNamespace(value));
    }
    try {
        const resolved = resolveVirtual(id, filename, ['import', 'node', 'default']);
        if (!resolved) return Promise.reject(new Error(`Cannot find module '${id}'`));
        const value = loadVirtualModule(resolved, false);
        return Promise.resolve(commonJsNamespace(value));
    } catch (error) {
        if (error?.code === 'MODULE_NOT_FOUND') error.code = 'ERR_MODULE_NOT_FOUND';
        return Promise.reject(error);
    }
}

globalThis.__mcpV8ImportVirtualModule = importVirtualModule;

function originalEsmSource(source) {
    if (!source.startsWith(ORIGINAL_ESM_PREFIX)) return null;
    const end = source.indexOf('*/', ORIGINAL_ESM_PREFIX.length);
    if (end < 0) return null;
    return decodeURIComponent(source.slice(ORIGINAL_ESM_PREFIX.length, end));
}

function requireModuleEnabled() {
    const flags = globalThis.__NODE_TEST_FLAGS__;
    return !Array.isArray(flags) || !flags.includes('--no-experimental-require-module');
}

function loadVirtualModule(specifier, includeRequireCache = true) {
    const source = virtualCommonJsModules[specifier];
    if (source === undefined) return undefined;
    const originalEsm = originalEsmSource(source);
    if (includeRequireCache && originalEsm !== null && !requireModuleEnabled()) {
        Function(originalEsm);
    }
    if (virtualModuleCache.has(specifier)) {
        const module = virtualModuleCache.get(specifier);
        if (includeRequireCache) requireCache[module.filename] = module;
        return module.exports;
    }
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
    if (includeRequireCache) requireCache[filename] = module;
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
        const request = String(id);
        if (request.startsWith(ESM_IMPORT_PREFIX)) {
            const importId = request.slice(ESM_IMPORT_PREFIX.length);
            const name = importId.replace(/^node:/, '');
            if (builtins.has(name)) return builtins.get(name);
            const resolved = resolveVirtual(importId, _filename, ['import', 'node', 'default']);
            if (resolved) return loadVirtualModule(resolved, false);
            const err = new Error("Cannot find module '" + importId + "'");
            err.code = 'MODULE_NOT_FOUND';
            throw err;
        }
        const name = request.replace(/^node:/, '');
        if (builtins.has(name)) return builtins.get(name);
        const resolved = resolveVirtual(request, _filename);
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
    require.cache = requireCache;
    require.main = undefined;
    return require;
}

export function syncBuiltinESMExports() {
    // Builtins here are plain ESM with live bindings; nothing to sync.
}

const moduleHooks = globalThis.__mcpV8ModuleHooks ??= [];

export function register(specifier, parentURL = 'file:///') {
    const options = parentURL && typeof parentURL === 'object' && !(parentURL instanceof URL)
        ? parentURL
        : { parentURL };
    const base = options.parentURL instanceof URL
        ? options.parentURL.href
        : String(options.parentURL || 'file:///');
    const request = String(specifier);
    const relative = request.startsWith('.') || request.startsWith('/') || request.startsWith('file:');
    const resolved = relative ? new URL(request, base).href : request;
    const pending = globalThis.__mcpV8PendingModuleRegistrations ??= [];
    const registration = import(resolved).then((hooks) => registerHooks(hooks));
    pending.push(registration);
}

export function registerHooks(hooks) {
    if (hooks === null || typeof hooks !== 'object') {
        throw new TypeError('hooks must be an object');
    }
    const registered = {
        resolve: typeof hooks.resolve === 'function' ? hooks.resolve : undefined,
        load: typeof hooks.load === 'function' ? hooks.load : undefined,
    };
    moduleHooks.push(registered);
    let active = true;
    return {
        deregister() {
            if (!active) return;
            active = false;
            const index = moduleHooks.indexOf(registered);
            if (index >= 0) moduleHooks.splice(index, 1);
        },
    };
}

function resolveImportMetaSpecifier(specifier, parentURL) {
    if (parentURL instanceof URL) parentURL = parentURL.href;
    else if (typeof parentURL !== 'string') {
        const error = new TypeError('The "parent" argument must be of type string or an instance of URL.');
        error.code = 'ERR_INVALID_ARG_TYPE';
        throw error;
    }
    const defaultResolve = (nextSpecifier, context) => {
        const request = String(nextSpecifier);
        const builtin = request.replace(/^node:/, '');
        if (isBuiltin(request)) return { url: `node:${builtin}` };
        const hasScheme = /^[A-Za-z][A-Za-z0-9+.-]*:/.test(request);
        const relative = request.startsWith('.') || request.startsWith('/') ||
            request.startsWith('file:') || request.startsWith('data:');
        if (context.parentURL.startsWith('data:') && !hasScheme) {
            const error = new Error(
                `Failed to resolve module specifier "${request}" from "${context.parentURL}"`);
            error.code = 'ERR_UNSUPPORTED_RESOLVE_REQUEST';
            throw error;
        }
        if (relative || hasScheme) {
            return { url: relative ? new URL(request, context.parentURL).href : request };
        }
        if (!context.parentURL.startsWith('file:')) {
            const error = new TypeError(`Invalid URL: ${context.parentURL}`);
            error.name = 'TypeError [ERR_INVALID_URL]';
            error.code = 'ERR_INVALID_URL';
            throw error;
        }
        const resolved = resolveImportSpecifier(request, context.parentURL);
        if (resolved) {
            return { url: request.endsWith('/') ? new URL('.', resolved).href : resolved };
        }
        const error = new Error(`Cannot find package '${request}' imported from ${context.parentURL}`);
        error.code = 'ERR_MODULE_NOT_FOUND';
        throw error;
    };
    const context = { conditions: ['node', 'import'], importAttributes: {}, parentURL };
    const hooks = moduleHooks
        .map((hook) => hook.resolve)
        .filter((hook) => typeof hook === 'function');
    const run = (index, request, nextContext) => {
        if (index < 0) return defaultResolve(request, nextContext);
        return hooks[index](request, nextContext,
            (nextSpecifier = request, forwardedContext = nextContext) =>
                run(index - 1, nextSpecifier, forwardedContext));
    };
    const result = run(hooks.length - 1, String(specifier), context);
    if (result === null || typeof result !== 'object' || typeof result.url !== 'string') {
        throw new TypeError('resolve hook must return an object with a URL');
    }
    return result.url;
}

globalThis.__mcpV8ImportMetaResolve = resolveImportMetaSpecifier;

export class Module {
    constructor(id = '', parent = undefined) {
        this.id = id;
        this.path = '';
        this.exports = {};
        this.filename = null;
        this.loaded = false;
        this.parent = parent;
        this.children = [];
        this.paths = [];
    }

    require(id) {
        return createRequire(this.filename || 'file:///')(id);
    }
}

Object.assign(Module, {
    Module,
    builtinModules,
    isBuiltin,
    createRequire,
    syncBuiltinESMExports,
    register,
    registerHooks,
    _cache: requireCache,
});
builtins.set('module', Module);

export default Module;
