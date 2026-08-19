// WebAssembly.compileStreaming / instantiateStreaming over Response.
(function () {
    'use strict';
    if (typeof WebAssembly !== 'object' || WebAssembly === null) return;

    // Align V8's native WebAssembly.Global value setter with the current
    // JS-API spec's argument handling.
    try {
        var desc = Object.getOwnPropertyDescriptor(WebAssembly.Global.prototype, 'value');
        if (desc && desc.set && !desc.set.__mcpV8Arity) {
            var nativeSet = desc.set;
            // Per the current JS-API spec, an argument-less call behaves
            // as if undefined were passed; native V8 throws instead.
            var wrappedSet = function (v) {
                return nativeSet.call(this, arguments.length < 1 ? undefined : v);
            };
            // WebIDL names accessor functions "set value".
            Object.defineProperty(wrappedSet, 'name', { value: 'set value', configurable: true });
            Object.defineProperty(wrappedSet, '__mcpV8Arity', { value: true });
            Object.defineProperty(WebAssembly.Global.prototype, 'value', {
                get: desc.get,
                set: wrappedSet,
                enumerable: desc.enumerable,
                configurable: true,
            });
        }
    } catch (_) { /* best effort */ }

    if (typeof WebAssembly.compileStreaming === 'function') return;

    function bytesFromResponse(source) {
        return Promise.resolve(source).then(function (response) {
            if (typeof Response === 'function' && response instanceof Response) {
                if (!response.ok) {
                    throw new TypeError(
                        'WebAssembly response has unsupported status: ' + response.status);
                }
                var mime = String(response.headers.get('content-type') || '')
                    .split(';')[0].trim().toLowerCase();
                if (mime !== 'application/wasm') {
                    throw new TypeError(
                        "WebAssembly response has unsupported MIME type '" + mime + "'");
                }
                return response.arrayBuffer();
            }
            throw new TypeError(
                'An argument must be provided, which must be a Response or Promise<Response> object');
        });
    }

    Object.defineProperty(WebAssembly, 'compileStreaming', {
        value: function compileStreaming(source) {
            return bytesFromResponse(source).then(function (bytes) {
                return WebAssembly.compile(bytes);
            });
        },
        writable: true,
        enumerable: true,
        configurable: true,
    });

    Object.defineProperty(WebAssembly, 'instantiateStreaming', {
        value: function instantiateStreaming(source, importObject) {
            return bytesFromResponse(source).then(function (bytes) {
                return WebAssembly.instantiate(bytes, importObject);
            });
        },
        writable: true,
        enumerable: true,
        configurable: true,
    });
})();
