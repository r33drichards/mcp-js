// Bootstrap for running WPT testharness.js tests inside the mcp-v8 engine.
//
// Runs before testharness.js. Provides just enough of a "shell" environment
// that testharness selects its ShellTestEnvironment (no document, no worker
// scopes) and that `.any.js` tests can sniff their scope via self.GLOBAL.
//
// __WPT_TEST_PATH__ is injected by the Rust harness before this file.

globalThis.self = globalThis;

globalThis.GLOBAL = {
  isWindow: function () { return false; },
  isWorker: function () { return false; },
  isShadowRealm: function () { return false; },
};

// Minimal location mock (Node's WPT runner does the same). Gives testharness
// a stable default title and lets tests read location.pathname.
(function () {
  var path = globalThis.__WPT_TEST_PATH__ || "/unknown.any.js";
  // Plain object, NOT a URL instance (url/historical.any.js asserts location
  // has no searchParams), but stringifiable so `new URL(x, location)` works.
  var href = "http://web-platform.test:8000" + path;
  globalThis.location = {
    href: href,
    pathname: path,
    search: "",
    hash: "",
    toString: function () { return href; },
  };
})();

// Serve embedded test resources (resources/*.json, injected by the Rust
// harness as __WPT_RESOURCES__) through fetch, so data-driven tests work
// without a wptserve. Everything else falls through to the real fetch.
(function () {
  var resources = globalThis.__WPT_RESOURCES__ || {};
  var assets = globalThis.__WPT_ASSETS__ || {};
  if (Object.keys(resources).length === 0 && Object.keys(assets).length === 0) return;
  var realFetch = globalThis.fetch;
  function embeddedResponse(text) {
    return Promise.resolve({
      ok: true,
      status: 200,
      json: function () { return Promise.resolve(JSON.parse(text)); },
      text: function () { return Promise.resolve(text); },
      bytes: function () { return Promise.resolve(new TextEncoder().encode(text)); },
      arrayBuffer: function () {
        return Promise.resolve(new TextEncoder().encode(text).buffer);
      },
    });
  }
  globalThis.fetch = function fetch(input, init) {
    var key = String(input);
    if (Object.prototype.hasOwnProperty.call(resources, key)) {
      return embeddedResponse(resources[key]);
    }
    if (Object.prototype.hasOwnProperty.call(assets, key)) {
      return embeddedResponse(assets[key]);
    }
    if (typeof realFetch === "function") {
      return realFetch.apply(this, arguments);
    }
    return Promise.reject(new TypeError("fetch is not available"));
  };
})();
