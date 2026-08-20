# Node Compatibility Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the versioned corpus, expectation, inventory, reporting, and command infrastructure for expanding `mcp-v8` Node.js compatibility without changing runtime APIs.

**Architecture:** The existing Rust Node-core harness remains the executable fast suite and gains a typed expectation schema plus family/profile filters. Dependency-free Python tools fetch pinned external corpora, generate a deterministic inventory, validate metadata, and produce committed JSON/Markdown reports. A small shell entrypoint and CI drift checks expose reproducible fast, filtered, inventory, and report workflows.

**Tech Stack:** Rust integration tests with Serde, Python 3 standard library, Bash, GitHub Actions, JSON, Markdown.

## Global Constraints

- Keep the default web-compatible runtime and security posture unchanged.
- Do not modify runtime APIs in this pull request.
- Keep Node core behavior pinned to `v22.14.0`.
- Pin `denoland/node_test` commit `f287abd897685505b021996828004a917f715a0f`, which contains Node `26.5.1`, as a separately versioned inventory corpus.
- Verify the Deno corpus archive SHA-256 `9ad1bdd6b546a8cbb0be30743dae2b20a2215abda8d125418ffa7c3c41104c92`.
- Pin CITGM commit `09ca92bb5f1ed77e1aec3bed129285c1568797eb` and archive SHA-256 `d56c70ea7f4f8f3522c77c7b21788629a1b63c6a220e9108eea6d5c1f7f0b4b1` as future ecosystem metadata.
- Preserve the existing 35 passing and two skipped Node tests.
- Do not commit downloaded external corpora.
- Use only Python's standard library for compatibility tooling.
- Preserve upstream test sources without edits.

---

### Task 1: Typed Expectations And Harness Filters

**Files:**
- Modify: `server/tests/node_compat.rs`
- Modify: `server/tests/node_compat/expectations.json`

**Interfaces:**
- Consumes: Existing `Outcome`, `NODE_COMPAT_FILTER`, and vendored `test/parallel` subset.
- Produces: `Expectation { status, family, profile, compatibility, reason, expires }`, `ExpectationStatus`, and `CompatibilityLevel`; environment filters `NODE_COMPAT_FAMILY` and `NODE_COMPAT_PROFILE`.

- [ ] **Step 1: Add failing unit tests for the new schema**

Add `#[cfg(test)] mod expectation_tests` to `server/tests/node_compat.rs` with tests that parse:

```rust
#[test]
fn parses_passing_expectation_metadata() {
    let value = r#"{
      "status":"pass",
      "family":"events",
      "profile":"pure",
      "compatibility":"exact"
    }"#;
    let parsed: Expectation = serde_json::from_str(value).unwrap();
    assert_eq!(parsed.status, ExpectationStatus::Pass);
    assert_eq!(parsed.family, "events");
    assert_eq!(parsed.profile, "pure");
    assert_eq!(parsed.compatibility, CompatibilityLevel::Exact);
}

#[test]
fn rejects_non_pass_without_reason() {
    let value = r#"{
      "status":"harness_missing",
      "family":"events",
      "profile":"pure",
      "compatibility":"unsupported"
    }"#;
    let parsed: Expectation = serde_json::from_str(value).unwrap();
    assert!(parsed.validate().is_err());
}

#[test]
fn filter_matches_family_and_profile() {
    let expectation = Expectation::passing("events", "pure");
    assert!(expectation.matches(Some("events"), Some("pure")));
    assert!(!expectation.matches(Some("streams"), Some("pure")));
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cd server
cargo test --test node_compat expectation_tests -- --nocapture
```

Expected: compilation fails because the new expectation types and methods do not exist.

- [ ] **Step 3: Implement the typed expectation model**

Replace the untagged boolean/detail schema with:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExpectationStatus {
    Pass,
    Fail,
    Unsupported,
    HarnessMissing,
    PolicyRequired,
    Flaky,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CompatibilityLevel {
    Exact,
    Adapted,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Expectation {
    status: ExpectationStatus,
    family: String,
    profile: String,
    compatibility: CompatibilityLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires: Option<String>,
}
```

Implement:

```rust
impl Expectation {
    fn passing(family: &str, profile: &str) -> Self;
    fn validate(&self) -> Result<(), String>;
    fn runnable(&self) -> bool;
    fn matches(&self, family: Option<&str>, profile: Option<&str>) -> bool;
}
```

Validation rules:

- `pass` requires no reason.
- Every other status requires a non-empty reason.
- `flaky` also requires a non-empty ISO date in `expires`.
- `exact` and `adapted` are runnable compatibility levels.
- `unsupported` compatibility cannot use `status: pass`.

Update the harness so:

- `pass` expects `Outcome::Pass`.
- `fail` expects `Outcome::Fail`.
- `unsupported`, `harness_missing`, `policy_required`, and `flaky` are not run by the fast suite.
- `NODE_COMPAT_FAMILY` and `NODE_COMPAT_PROFILE` select matching expectations.
- `NODE_COMPAT_FILTER` continues to filter by path substring.
- Update mode preserves family/profile/compatibility metadata and changes only `status` between `pass` and `fail`.

- [ ] **Step 4: Migrate the existing expectation file**

Convert all 35 passing entries to objects with `status: "pass"`, `profile: "pure"`, and `compatibility: "exact"`. Assign families from file names: `console`, `crypto`, `events`, `path`, `querystring`, and `timers`.

Convert:

```json
"test/parallel/test-events-once.js": {
  "status": "harness_missing",
  "family": "events",
  "profile": "pure",
  "compatibility": "unsupported",
  "reason": "requires node-internal module internal/event_target"
}
```

and:

```json
"test/parallel/test-path-resolve.js": {
  "status": "policy_required",
  "family": "path",
  "profile": "subprocess",
  "compatibility": "adapted",
  "reason": "requires child_process to verify cwd-dependent resolution"
}
```

- [ ] **Step 5: Run focused and baseline tests and verify GREEN**

Run:

```bash
cd server
cargo test --test node_compat expectation_tests -- --nocapture
cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture
NODE_COMPAT_FAMILY=events cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture
```

Expected: schema tests pass; baseline reports 35 runnable passing tests and two classified non-runnable tests; family filtering runs only event tests.

- [ ] **Step 6: Commit Task 1**

```bash
git add server/tests/node_compat.rs server/tests/node_compat/expectations.json
git commit -m "test: classify Node compatibility expectations"
```

---

### Task 2: Reproducible External Corpus Tooling

**Files:**
- Modify: `.gitignore`
- Modify: `server/tests/node_compat/versions.json`
- Create: `tools/compat/node_compat_common.py`
- Create: `tools/compat/fetch-node-compat-corpora.py`
- Create: `tools/compat/tests/test_node_compat_tools.py`

**Interfaces:**
- Consumes: Exact pins and SHA-256 values in `versions.json`.
- Produces: `load_versions(path)`, `download_and_verify(source, cache_dir)`, and extracted corpora under `.cache/node-compat/<source>-<commit>/`.

- [ ] **Step 1: Write failing Python tests for metadata and archive verification**

Create `tools/compat/tests/test_node_compat_tools.py` using `unittest`. Tests must:

- load a fixture metadata object and require `repository`, `commit`, `archive_url`, `sha256`, and source version;
- reject a 63-character or non-hex SHA-256;
- verify a locally created tar archive against its checksum;
- reject checksum mismatch before extraction;
- reject archive members containing `..` or absolute paths; and
- extract a valid archive into a deterministic destination.

Load `tools/compat/node_compat_common.py` through `importlib.util.spec_from_file_location` so hyphenated command scripts remain executable files rather than import modules.

- [ ] **Step 2: Run Python tests and verify RED**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
```

Expected: import fails because `node_compat_common.py` does not exist.

- [ ] **Step 3: Add version metadata**

Expand `server/tests/node_compat/versions.json` to:

```json
{
  "node": {
    "tag": "v22.14.0",
    "repository": "https://github.com/nodejs/node",
    "vendored_by": "tools/compat/vendor-node-tests.sh"
  },
  "deno_node_test": {
    "repository": "https://github.com/denoland/node_test",
    "commit": "f287abd897685505b021996828004a917f715a0f",
    "node_version": "26.5.1",
    "archive_url": "https://codeload.github.com/denoland/node_test/tar.gz/f287abd897685505b021996828004a917f715a0f",
    "sha256": "9ad1bdd6b546a8cbb0be30743dae2b20a2215abda8d125418ffa7c3c41104c92"
  },
  "citgm": {
    "repository": "https://github.com/nodejs/citgm",
    "commit": "09ca92bb5f1ed77e1aec3bed129285c1568797eb",
    "archive_url": "https://codeload.github.com/nodejs/citgm/tar.gz/09ca92bb5f1ed77e1aec3bed129285c1568797eb",
    "sha256": "d56c70ea7f4f8f3522c77c7b21788629a1b63c6a220e9108eea6d5c1f7f0b4b1"
  }
}
```

Document through field names that the Deno corpus targets Node 26.5.1 and is not the Node 22 executable baseline.

- [ ] **Step 4: Implement secure standard-library fetch helpers**

`node_compat_common.py` must provide:

```python
def load_versions(path: pathlib.Path) -> dict[str, dict[str, str]]
def validate_source(name: str, source: dict[str, str]) -> None
def sha256_file(path: pathlib.Path) -> str
def safe_extract_tar(archive: pathlib.Path, destination: pathlib.Path) -> None
def download_and_verify(name: str, source: dict[str, str], cache_dir: pathlib.Path) -> pathlib.Path
```

Use `urllib.request`, `hashlib`, `tarfile`, temporary files, and atomic rename. Validate every archive member resolves below the destination before extraction.

- [ ] **Step 5: Implement the corpus command**

`fetch-node-compat-corpora.py` accepts:

```text
--source deno_node_test|citgm|all
--cache-dir PATH
--offline
```

Default cache: `.cache/node-compat`. It prints the extracted directory for each source. `--offline` succeeds only when a verified cached archive and extracted directory exist.

Add `.cache/node-compat/` to `.gitignore`.

- [ ] **Step 6: Run tests and a real download verification**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
python3 tools/compat/fetch-node-compat-corpora.py --source deno_node_test
python3 tools/compat/fetch-node-compat-corpora.py --source deno_node_test --offline
```

Expected: unit tests pass; online and offline commands print the same extracted directory; no corpus files appear in `git status`.

- [ ] **Step 7: Commit Task 2**

```bash
git add .gitignore server/tests/node_compat/versions.json tools/compat/node_compat_common.py tools/compat/fetch-node-compat-corpora.py tools/compat/tests/test_node_compat_tools.py
git commit -m "test: pin Node compatibility corpora"
```

---

### Task 3: Inventory And Compatibility Reports

**Files:**
- Create: `tools/compat/gen-node-compat-inventory.py`
- Create: `tools/compat/gen-node-compat-report.py`
- Create: `server/tests/node_compat/inventory.json`
- Create: `server/tests/node_compat/report.json`
- Create: `site-docs/reference/node-compatibility-status.md`
- Modify: `tools/compat/tests/test_node_compat_tools.py`
- Modify: `tools/compat/gen-compat-docs.py`
- Modify: `site-docs/reference/compatibility.md`

**Interfaces:**
- Consumes: extracted Deno corpus, versions metadata, and typed fast-suite expectations.
- Produces: deterministic inventory entries and aggregate JSON/Markdown reports.

- [ ] **Step 1: Add failing inventory classification tests**

Extend `test_node_compat_tools.py` with temporary corpus fixtures and subprocess calls asserting:

- `test/parallel/test-stream-readable.js` classifies as family `streams`, profile `pure`;
- `test/parallel/test-fs-read-file.js` classifies as `filesystem`, profile `filesystem`;
- `test/parallel/test-child-process-exec.js` classifies as `subprocess`, profile `subprocess`;
- `test/parallel/test-net-server.js` classifies as `networking`, profile `network-server`;
- `test/parallel/test-worker-message-port.js` classifies as `workers`, profile `workers`;
- addon, inspector, and embedding paths classify as `unsupported` with a reason; and
- output order is stable by path.

Add report tests asserting totals by status, family, profile, and compatibility level.

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
```

Expected: subprocess calls fail because inventory and report generators do not exist.

- [ ] **Step 3: Implement inventory generation**

`gen-node-compat-inventory.py` accepts:

```text
--corpus PATH
--output server/tests/node_compat/inventory.json
--check
```

Scan `.js`, `.mjs`, and `.cjs` files under `test/parallel`, `test/sequential`, and `test/es-module`. Emit:

```json
{
  "schema_version": 1,
  "source": {
    "name": "deno_node_test",
    "commit": "...",
    "node_version": "26.5.1"
  },
  "tests": [
    {
      "path": "test/parallel/test-stream-readable.js",
      "family": "streams",
      "profile": "pure",
      "status": "untriaged",
      "compatibility": "unsupported",
      "reason": "not yet selected for the mcp-v8 runner"
    }
  ]
}
```

Use ordered filename/path rules for families: `assert`, `buffer`, `console`, `crypto`, `dns`, `events`, `filesystem`, `http`, `module`, `networking`, `os`, `path`, `process`, `querystring`, `streams`, `subprocess`, `timers`, `tls`, `url`, `util`, `vm`, `workers`, and `other`.

Use profiles: `pure`, `filesystem`, `subprocess`, `network-client`, `network-server`, `workers`, `inspector`, and `native`.

- [ ] **Step 4: Implement report generation**

`gen-node-compat-report.py` reads expectations, inventory, and versions. It writes:

- `server/tests/node_compat/report.json` with deterministic aggregate counts;
- `site-docs/reference/node-compatibility-status.md` with version pins, fast-suite results, full-corpus inventory totals, family/profile tables, and explicit Node 22 versus Deno Node 26 corpus labels.

The report must never combine the two versions into one compatibility percentage.

Update `gen-compat-docs.py` to understand the typed expectation schema and retain the existing compatibility page's 35/35 passing statement plus classified skipped-test reasons.

- [ ] **Step 5: Generate committed artifacts**

Run:

```bash
CORPUS=$(python3 tools/compat/fetch-node-compat-corpora.py --source deno_node_test --offline | tail -1)
python3 tools/compat/gen-node-compat-inventory.py --corpus "$CORPUS"
python3 tools/compat/gen-node-compat-report.py
python3 tools/compat/gen-compat-docs.py
```

Expected inventory counts include 4,166 `test/parallel` JavaScript files, 110 `test/sequential` files, and 100 `test/es-module` JavaScript files from the pinned corpus; generated files are stable on a second run.

- [ ] **Step 6: Run tests and drift checks**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
python3 tools/compat/gen-node-compat-inventory.py --corpus "$CORPUS" --check
python3 tools/compat/gen-node-compat-report.py --check
python3 tools/compat/gen-compat-docs.py
git diff --check
```

Expected: all tests pass and `--check` reports no drift.

- [ ] **Step 7: Commit Task 3**

```bash
git add tools/compat/gen-node-compat-inventory.py tools/compat/gen-node-compat-report.py tools/compat/gen-compat-docs.py tools/compat/tests/test_node_compat_tools.py server/tests/node_compat/inventory.json server/tests/node_compat/report.json site-docs/reference/node-compatibility-status.md site-docs/reference/compatibility.md
git commit -m "test: inventory Node compatibility coverage"
```

---

### Task 4: Commands, Contributor Documentation, And CI Drift

**Files:**
- Create: `tools/compat/node-compat.sh`
- Create: `server/tests/node_compat/README.md`
- Modify: `.github/workflows/docs-check.yml`
- Modify: `site-docs/reference/node-compatibility-status.md`
- Modify: `tools/compat/tests/test_node_compat_tools.py`

**Interfaces:**
- Consumes: corpus fetcher, inventory/report generators, Rust harness filters.
- Produces: stable commands `fetch`, `inventory`, `report`, `fast`, `family`, `profile`, and `check`.

- [ ] **Step 1: Add failing command-contract tests**

Add Python tests that execute `tools/compat/node-compat.sh --help` and assert it documents:

```text
fetch
inventory
report
fast
family <name>
profile <name>
check
```

Use a fake `cargo` executable prepended to `PATH` to verify:

- `fast` invokes `cargo test --test node_compat node_core_subset_matches_expectations`;
- `family events` sets `NODE_COMPAT_FAMILY=events`;
- `profile pure` sets `NODE_COMPAT_PROFILE=pure`; and
- missing family/profile values exit non-zero.

- [ ] **Step 2: Run command tests and verify RED**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
```

Expected: tests fail because `node-compat.sh` does not exist.

- [ ] **Step 3: Implement the command wrapper**

The Bash script uses `set -euo pipefail`, resolves the repository root, and implements:

```text
fetch      fetch the pinned Deno and CITGM corpora
inventory  fetch Deno if needed and regenerate inventory
report     regenerate JSON and Markdown reports
fast       run the current curated Rust suite
family     run curated tests matching NODE_COMPAT_FAMILY
profile    run curated tests matching NODE_COMPAT_PROFILE
check      run Python tests, Rust fast suite, inventory drift, report drift, and docs drift
```

All Cargo commands run from `server/`. `check` reuses the verified cached Deno corpus and does not download when `--offline` is requested through `NODE_COMPAT_OFFLINE=1`.

- [ ] **Step 4: Document contributor workflow**

Create `server/tests/node_compat/README.md` documenting:

- the Node 22 fast suite versus Deno Node 26 inventory distinction;
- expectation statuses and compatibility levels;
- capability profiles;
- exact local commands;
- how to select an upstream test family;
- the required red-to-green workflow for moving `harness_missing` or `unsupported` tests to `fail`, then `pass`;
- generated-file update commands; and
- why downloaded corpora are ignored.

- [ ] **Step 5: Add CI drift checks**

Update `.github/workflows/docs-check.yml` to run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
python3 tools/compat/gen-node-compat-report.py --check
```

Keep full corpus downloads out of pull-request docs CI. Inventory drift is verified from the committed inventory and source metadata; scheduled full-corpus execution remains a follow-up.

- [ ] **Step 6: Run focused verification**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
tools/compat/node-compat.sh fast
tools/compat/node-compat.sh family events
tools/compat/node-compat.sh profile pure
tools/compat/node-compat.sh report
git diff --check
```

Expected: all commands pass and generated reports do not drift.

- [ ] **Step 7: Commit Task 4**

```bash
git add tools/compat/node-compat.sh tools/compat/tests/test_node_compat_tools.py server/tests/node_compat/README.md .github/workflows/docs-check.yml site-docs/reference/node-compatibility-status.md
git commit -m "docs: add Node compatibility workflow"
```

---

### Task 5: Full Verification And Pull Request

**Files:**
- Modify only files required by verification fixes.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: a pushed `codex/node-compat-program` branch and a pull request against `main`.

- [ ] **Step 1: Run formatting**

Run:

```bash
cd server
cargo fmt --check
```

If it fails, run `cargo fmt`, inspect the diff, and rerun `cargo fmt --check`.

- [ ] **Step 2: Run focused compatibility verification**

Run:

```bash
python3 -m unittest tools/compat/tests/test_node_compat_tools.py -v
tools/compat/node-compat.sh check
```

Expected: Python tooling tests, Rust Node compatibility tests, and generated-file drift checks pass.

- [ ] **Step 3: Run broader repository tests**

Run:

```bash
cd server
cargo test --test node_compat -- --nocapture
cargo test --test node_builtins -- --nocapture
cargo test --test compat_surface -- --nocapture
```

Then run the repository's normal suite when resources permit:

```bash
cargo test -- --test-threads=1
```

Document any unrelated pre-existing failure without modifying unrelated code.

- [ ] **Step 4: Review the final diff**

Run:

```bash
git status --short
git diff main...HEAD --check
git diff main...HEAD --stat
git log --oneline --decorate main..HEAD
```

Expected: only compatibility-program files are changed; the worktree is clean after commits.

- [ ] **Step 5: Request code review locally**

Use the `requesting-code-review` skill to review behavior, security boundaries, generated artifacts, and test coverage. Address confirmed findings with focused commits and rerun affected verification.

- [ ] **Step 6: Push and open the pull request**

Push:

```bash
git push -u origin codex/node-compat-program
```

Open a PR against `main` using the available GitHub API or CLI. The PR body must include:

- the three-track program context;
- what this foundational PR implements;
- explicit Node 22 versus Deno Node 26 corpus labeling;
- security/non-runtime scope;
- verification commands and results; and
- follow-up module-family, differential, and CITGM work.
