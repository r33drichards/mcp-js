// WebAssembly.compileStreaming / instantiateStreaming over Response.
(function () {
    'use strict';
    if (typeof WebAssembly !== 'object' || WebAssembly === null) return;
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
