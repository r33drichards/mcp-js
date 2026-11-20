# Release upgrade notes

This guide describes the current `main` implementation. Check the selected
release's notes and assets before upgrading; a merged feature is not proof that
a particular platform artifact has finished publishing.

## Persistence is opt-in

Heap and filesystem snapshots are separate selectors, both defaulting to `none`.
Use `--heap-store dir|s3` and `--fs-store dir|s3` explicitly. The old
`--stateless` and `--directory-path` flags are removed. Replace them with
`--heap-store none` and `--heap-store dir --heap-dir PATH`, respectively.
`--s3-bucket` alone does not enable either snapshot axis.

WASM modules require `--heap-store none`. Filesystem persistence can remain on;
it does not automatically preserve a guest WASM database's linear memory.

## Coordinate runtime storage readers

Server artifact/console bytes now live in S3 or local files, with typed,
content-addressed references in versioned database indexes. New code reads
mainline legacy inline records and old explicit DynamoDB blob envelopes. Old
binaries cannot read the new indexes. Upgrade all readers before permitting
new-format writes; rollback requires compatible metadata/object formats.

Preserve the metadata and its selected blob store together. Neither changing
`--sqlite-path`/`--dynamodb-table` nor changing the bucket/path performs migration.
There is no size-based offload fallback for oversized metadata. See
[metadata configuration](../how-to/metadata-backends.md) and
[runtime storage](../concepts/runtime-storage.md).

## Match heap snapshots to the runtime

The runtime uses `rusty_v8` 152.2.0. V8 startup snapshots are runtime-specific;
a valid payload checksum does not establish compatibility with another V8
version, flags, or extensions. Keep a compatible runtime and tested restore
procedure for retained heaps. Incompatible snapshots fail validation. Fresh
execution module URLs avoid reusing a saved module's identity after restoration.

## Keep scheduler deployment boundaries explicit

The scheduler is merged but experimental. It uses checkpoint schema v2 and
worker protocol v1, dedicated workers, fixed voters, and preserved local/consensus
stores. Unsupported old checkpoints fail closed; there is no automatic migration.
Already admitted work can outlive takeover. `Unknown` is not automatically
replayed, and cancellation is confirmed only after the work stops.

Use the [deployment guide](../how-to/scheduler-http-nixos.md),
[recovery guide](../how-to/stop-scheduled-work.md), and
[production-readiness tracker](https://github.com/r33drichards/mcp-js/issues/348).
MCP Events delivery and exactly-once external effects are not implemented promises.

## Select the native SDK artifact separately

The native JavaScript SDK's glibc and musl assets target Linux x64. The standard
asset requires glibc 2.39+; Alpine uses the explicit musl tarball. These are
separate from server binaries and the HTTP client. Use the
[native SDK guide](../how-to/native-javascript-sdk.md) for host tools, lifecycle,
installation, and the current pinned-V8 build recipe.

## Release preparation checks

Build the docs with strict warnings, run generated-reference drift checks, check
links and browser rendering, and run the relevant storage/scheduler/SDK suites.
The [development guide](../how-to/development-validation.md) links reproducible
build and performance checks. Treat historical PR test counts as historical;
record the exact revision and outcomes used for the release decision.
