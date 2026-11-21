# Configure heap storage

Heap snapshots are disabled by default. Enable them explicitly; filesystem
snapshots and runtime payload storage are separate choices.

## Store heaps in a directory

```bash
mcp-v8 --http-port 8080 --heap-store dir \
  --heap-dir /var/lib/mcp-v8/heaps --session-db-path /var/lib/mcp-v8/sessions
```

The heap directory is created on write. If `--heap-dir` is omitted, an enabled
directory heap store uses `/tmp/mcp-v8-heaps`. Use a durable location for state
that must survive reboot.

## Store heaps in S3

Use an existing bucket and the normal AWS credential chain:

```bash
export AWS_REGION=us-east-1
mcp-v8 --http-port 8080 --heap-store s3 --s3-bucket my-mcp-blobs
```

All nodes sharing heap references must access the same objects. S3 errors are
returned; they do not cause a filesystem fallback. Setting `--s3-bucket` alone
selects S3 for runtime payloads but does not enable heap snapshots.

## Use an S3-compatible service

```bash
export AWS_ENDPOINT_URL=http://127.0.0.1:9000
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REGION=us-east-1
# Supply the service's credentials through the normal AWS environment/profile.
mcp-v8 --http-port 8080 --heap-store s3 --s3-bucket my-mcp-blobs
```

Create the bucket first. Unset the endpoint/path-style overrides when returning
to AWS S3.

## Add a local cache

```bash
mcp-v8 --http-port 8080 --heap-store s3 --s3-bucket my-mcp-blobs \
  --cache-dir /var/cache/mcp-v8/blobs
```

The cache is bounded to 512 MiB by default. Writes reach S3 before the local
cache; reads prefer complete cached files and populate on misses. Mutable writes
report cache-write errors after the primary succeeds; immutable runtime writes
treat that cache update as best-effort. See [storage behavior](../concepts/storage-backends.md).

## Disable heap persistence independently

```bash
mcp-v8 --http-port 8080 --heap-store none
```

This is the default. To retain filesystem state while leaving heaps disabled:

```bash
mcp-v8 --http-port 8080 --heap-store none --fs-store dir \
  --session-db-path /var/lib/mcp-v8/sessions
```

WASM requires heap persistence to be off and can still use filesystem snapshots.
With both snapshot axes off, the stateless MCP surface returns output inline;
execution metadata and artifact/console payload storage still exist.

## Select metadata separately

Use [metadata backend setup](metadata-backends.md) for SQLite and DynamoDB.
`--session-db-path` remains the parent for default local runtime blob storage,
even when metadata is in SQLite or DynamoDB. Back up the metadata and blobs
consistently; changing a selector does not migrate existing records.

## Source and related guides

- [Storage bootstrap](https://github.com/r33drichards/mcp-js/blob/main/server/src/bootstrap.rs)
- [Blob backends and cache](https://github.com/r33drichards/mcp-js/blob/main/server/src/engine/heap_storage.rs)
- [Filesystem snapshots](fs-snapshots.md)
- [Runtime payloads](../concepts/runtime-storage.md)
