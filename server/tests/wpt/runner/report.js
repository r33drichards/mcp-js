// Replacement for WPT's testharnessreport.js: serialize results over the
// engine's console channel. The Rust harness scans captured console output
// for the sentinel-prefixed line.
//
// Statuses (testharness.js): subtest 0=PASS 1=FAIL 2=TIMEOUT 3=NOTRUN
// 4=PRECONDITION_FAILED; harness 0=OK 1=ERROR 2=TIMEOUT 3=PRECONDITION_FAILED.

add_completion_callback(function (tests, harnessStatus) {
  var result = {
    status: harnessStatus.status,
    message: harnessStatus.message || null,
    tests: tests.map(function (t) {
      return {
        name: t.name,
        status: t.status,
        message: t.message || null,
      };
    }),
  };
  console.log("__WPT_RESULT__" + JSON.stringify(result));
});
