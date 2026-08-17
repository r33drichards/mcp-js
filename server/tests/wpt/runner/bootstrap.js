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
  globalThis.location = {
    href: "http://web-platform.test:8000" + path,
    pathname: path,
    search: "",
    hash: "",
  };
})();
