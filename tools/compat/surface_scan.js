// Phase-0 compat surface scan for the mcp-v8 runtime.
//
// Probes the globals of the WinterTC Minimum Common Web Platform API
// (https://min-common-api.proposal.wintertc.org/, Ecma TC55 "2025 snapshot")
// plus a few Node-ecosystem and mcp-v8-specific names, and dumps every own
// property of globalThis. Emits one sentinel-prefixed JSON line on the
// console channel; see server/tests/compat_surface.rs.

(function () {
  // WinterTC Minimum Common API globals (2025 snapshot draft).
  var MIN_COMMON_API = [
    // Events / messaging
    "AbortController", "AbortSignal", "Event", "EventTarget", "CustomEvent",
    "ErrorEvent", "MessageChannel", "MessageEvent", "MessagePort",
    "PromiseRejectionEvent", "DOMException",
    // Fetch / HTTP
    "fetch", "Headers", "Request", "Response", "FormData",
    // Files
    "Blob", "File",
    // Streams
    "ReadableStream", "ReadableStreamDefaultReader", "ReadableStreamBYOBReader",
    "WritableStream", "WritableStreamDefaultWriter", "TransformStream",
    "ByteLengthQueuingStrategy", "CountQueuingStrategy",
    // Compression
    "CompressionStream", "DecompressionStream",
    // Encoding
    "TextEncoder", "TextDecoder", "TextEncoderStream", "TextDecoderStream",
    "atob", "btoa",
    // URL
    "URL", "URLSearchParams", "URLPattern",
    // Crypto
    "crypto", "Crypto", "CryptoKey", "SubtleCrypto",
    // Performance
    "performance", "Performance",
    // WebAssembly (namespace presence probed separately below)
    "WebAssembly",
    // Misc globals
    "console", "navigator", "self",
    "setTimeout", "clearTimeout", "setInterval", "clearInterval",
    "queueMicrotask", "structuredClone", "reportError",
  ];

  // Non-WinterTC names worth tracking: Node-ecosystem shims agents rely on,
  // and mcp-v8's own host APIs.
  var EXTRAS = [
    "Buffer", "process", "require", "global",
    "SharedArrayBuffer", "Atomics",
    "fs", "child_process", "mcp", "McpToolError", "Deno",
  ];

  function probe(name) {
    try {
      var t = typeof globalThis[name];
      return t === "undefined" ? null : t;
    } catch (e) {
      return "throws:" + e.name;
    }
  }

  function probeList(names) {
    var out = {};
    for (var i = 0; i < names.length; i++) {
      out[names[i]] = probe(names[i]);
    }
    return out;
  }

  // WebAssembly namespace members per the Minimum Common API.
  var wasmMembers = [
    "Module", "Instance", "Memory", "Table", "Global", "Tag", "Exception",
    "CompileError", "LinkError", "RuntimeError",
    "compile", "instantiate", "validate",
    "compileStreaming", "instantiateStreaming",
  ];
  var wasm = {};
  if (typeof WebAssembly !== "undefined") {
    for (var i = 0; i < wasmMembers.length; i++) {
      var t = typeof WebAssembly[wasmMembers[i]];
      wasm[wasmMembers[i]] = t === "undefined" ? null : t;
    }
  }

  // Full own-property dump of globalThis (one level).
  var dump = {};
  var names = Object.getOwnPropertyNames(globalThis).sort();
  for (var j = 0; j < names.length; j++) {
    dump[names[j]] = probe(names[j]);
  }

  var minCommon = probeList(MIN_COMMON_API);
  var present = 0;
  var missing = [];
  for (var k in minCommon) {
    if (minCommon[k] !== null) present++;
    else missing.push(k);
  }

  var report = {
    minCommonApi: {
      total: MIN_COMMON_API.length,
      present: present,
      missing: missing,
      detail: minCommon,
    },
    webAssembly: wasm,
    extras: probeList(EXTRAS),
    globals: dump,
  };
  console.log("__SURFACE__" + JSON.stringify(report));
})();
