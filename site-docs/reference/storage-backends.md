# Heap storage backends

Complete reference for all flags that control heap snapshot storage and the sled session database.

## Storage backend flags

| Flag | Type | Default | Description |
|---|---|---|---|
| `--directory-path=DIR` | string | — | Store heap snapshots as files in `DIR`. The directory is created automatically. Conflicts with `--s3-bucket` and `--stateless`. |
| `--s3-bucket=NAME` | string | — | Store heap snapshots as objects in S3 bucket `NAME`. The bucket must already exist. Conflicts with `--directory-path` and `--stateless`. |
| `--cache-dir=DIR` | string | — | Add a local FS write-through cache in front of S3 at path `DIR`. Requires `--s3-bucket`; cannot be used alone. |
| `--stateless` | bool | `false` | Disable all heap persistence. Each execution runs in a fresh V8 isolate and state is not saved. Conflicts with `--s3-bucket` and `--directory-path`. |
| `--session-db-path=PATH` | string | `/tmp/mcp-v8-sessions` | Path for the sled embedded metadata database. The session log and heap tag index are not opened when `--stateless` is given, but the execution registry sub-path is still used. |

### Default backend

When none of `--directory-path`, `--s3-bucket`, or `--stateless` is given, the server defaults to `FileHeapStorage` at `/tmp/mcp-v8-heaps`. This path is **not** automatically persisted across reboots.

### Mutual-exclusivity rules

The following flags are mutually exclusive. Passing more than one is a startup error:

- `--s3-bucket` conflicts with `--directory-path`
- `--s3-bucket` conflicts with `--stateless`
- `--directory-path` conflicts with `--stateless`

`--cache-dir` requires `--s3-bucket` and conflicts with no other flag, but it is meaningless without S3.

## Session database (`--session-db-path`)

The sled database at `--session-db-path` (default `/tmp/mcp-v8-sessions`) stores four categories of data:

| Sub-path | Contents |
|---|---|
| `{session-db-path}` (root) | Session log — maps session IDs to execution sequences |
| `{session-db-path}/heap-tags` | Heap tag index — content-hash → arbitrary user-defined tags; supports `query_heaps_by_tags` |
| `{session-db-path}/executions` (or `{session-db-path}/executions-{port}` when `--http-port` is set) | Execution registry — status, output lines, and metadata for each execution ID; opened in both stateful and stateless modes |
| `{session-db-path}/cluster-{node-id}` | Per-node Raft state — only populated when `--cluster-port` is set |

The session DB is separate from the heap store. Losing the session DB does not destroy snapshot bytes; losing the heap store does not destroy session metadata.

## S3 credential expectations

The S3 client is initialised at server startup via `aws_config::load_from_env()`, which walks the standard AWS SDK credential chain in order:

1. **Environment variables**: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optionally `AWS_SESSION_TOKEN`.
2. **Shared credentials file**: `~/.aws/credentials` (profile selected by `AWS_PROFILE`, default `default`).
3. **IAM instance / task / workload identity**: EC2 instance metadata, ECS task credentials, or IRSA/web identity tokens.

Additional environment variables that influence S3 behaviour:

| Variable | Purpose |
|---|---|
| `AWS_DEFAULT_REGION` | AWS region for the bucket (required if not in `~/.aws/config`) |
| `AWS_ENDPOINT_URL` | Override the S3 endpoint URL (use for S3-compatible stores such as MinIO or LocalStack) |
| `AWS_SESSION_TOKEN` | Temporary session token for STS-issued credentials |

The server does not accept S3 credentials via CLI flags. All credential configuration must be done through the AWS SDK credential chain before the process starts.

The S3 bucket must already exist and the credentials must have `s3:PutObject` and `s3:GetObject` permissions on the bucket. The server does not create or configure the bucket.

## WriteThroughCacheHeapStorage behaviour

When both `--s3-bucket` and `--cache-dir` are given:

| Operation | Local cache | S3 |
|---|---|---|
| `put` | Written first | Written after cache; both must succeed |
| `get` (cache hit) | Read and returned | Not contacted |
| `get` (cache miss) | Populated from S3 result (warning logged on failure) | Fetched and returned |

A `put` failure on either the cache or S3 returns an error to the caller. A cache-population failure after an S3 `get` logs a warning but does not fail the operation.

## See also

- [How-to: Heap storage backends](../how-to/storage-backends.md)
- [Concepts: Heap storage backends](../concepts/storage-backends.md)
- [Concepts: Stateful sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Concepts: Clustering & replication (Raft)](../concepts/clustering.md)
- [CLI flags reference](cli-flags.md)
