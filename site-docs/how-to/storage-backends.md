# Heap storage backends

Recipes for selecting and configuring the heap snapshot store that `mcp-v8` uses in stateful mode.

## Use local directory storage

Pass `--directory-path` to store heap snapshots in a specific local directory:

```bash
mcp-v8 --http-port=8080 --directory-path=/var/lib/mcp-v8/heaps
```

The directory is created automatically if it does not exist. Omitting `--directory-path` (and all other storage flags) uses the default path `/tmp/mcp-v8-heaps`.

This backend is well-suited for single-node deployments where the heap directory lives on fast local storage (e.g. an SSD or a RAM-backed tmpfs).

## Use S3 storage

Set `--s3-bucket` to store heap snapshots in an S3 bucket:

```bash
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_DEFAULT_REGION=us-east-1

mcp-v8 --http-port=8080 --s3-bucket=my-mcp-heaps
```

The server initialises the S3 client from the environment at startup using the standard AWS SDK credential chain (see [reference](../reference/storage-backends.md) for the full list of variables). The bucket must already exist; the server does not create it.

S3 storage lets multiple nodes share the same heap store, which is required for a horizontally-scaled or clustered deployment.

## Add a local FS write-through cache in front of S3

Use `--cache-dir` together with `--s3-bucket` to keep a local disk cache:

```bash
mcp-v8 --http-port=8080 \
  --s3-bucket=my-mcp-heaps \
  --cache-dir=/var/cache/mcp-v8/heaps
```

With this configuration every `put` writes to the local cache directory first and then to S3 (both writes must succeed). Every `get` checks the local cache first; on a miss it fetches from S3, populates the cache, and returns the data. A warning is logged if the cache write after an S3 fetch fails, but the `get` still succeeds.

`--cache-dir` requires `--s3-bucket` and cannot be used alone.

## Run stateless (no heap persistence)

Pass `--stateless` to disable heap persistence entirely:

```bash
mcp-v8 --http-port=8080 --stateless
```

In stateless mode each `run_js` invocation executes in a fresh V8 isolate, waits synchronously for completion, and returns `{output}` or `{output, error}` directly. No heap snapshot is ever written. The session log and heap tag index are not opened; the execution registry sled database (under `--session-db-path`) is still used.

Stateless mode is appropriate for sandboxed, fire-and-forget executions where cross-call state is neither needed nor desired.

`--stateless` conflicts with `--s3-bucket` and `--directory-path`; passing more than one of these three flags is a startup error.

## Change the session metadata path

The sled metadata database (session log, heap tags, execution registry) is stored at `--session-db-path`, which defaults to `/tmp/mcp-v8-sessions`. Override it when you want the metadata to persist across reboots:

```bash
mcp-v8 --http-port=8080 \
  --directory-path=/var/lib/mcp-v8/heaps \
  --session-db-path=/var/lib/mcp-v8/sessions
```

This flag is independent of the heap storage backend selection and applies in both stateful and stateless modes.

## See also

- [Concepts: Heap storage backends](../concepts/storage-backends.md)
- [Reference: Heap storage backends](../reference/storage-backends.md)
- [Concepts: Stateful sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Concepts: Clustering & replication (Raft)](../concepts/clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
