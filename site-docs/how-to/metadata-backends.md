# Configure metadata backends

Choose a metadata database independently of heap and filesystem blobs. Sled is
the default. SQLite and DynamoDB cover session history, heap tags, filesystem
labels/reflogs, execution records, console descriptors, artifact metadata, and
upload grants. Records for disabled snapshot features are not attached. SQLite/DynamoDB persist
execution status across processes; the default sled registry keeps live status
in memory while retaining console/artifact data in its stores.

## Use embedded SQLite

Create the parent directory, then start the server:

```bash
mkdir -p ./data
mcp-v8 --http-port 8080 --sqlite-path ./data/metadata.sqlite \
  --heap-store none --fs-store dir --session-db-path ./data/local
```

`MCP_V8_SQLITE_PATH` and the config key `sqlite_path` select the same file.
Namespaces separate sessions, tags, labels, and runtime records. The runtime
namespace is stable across port changes. SQLite uses WAL, `synchronous=FULL`,
transactional compare-and-set, and a five-second busy timeout.

The file's parent must exist. An explicit backend open/probe failure aborts
startup rather than falling back to sled. Back up using SQLite's backup API or a
consistent database/WAL snapshot. This is server metadata SQLite, unrelated to
[running a guest SQLite WASM database](../tutorials/sqlite-wasm.md).

## Use shared DynamoDB

Create a table with String partition/sort keys `pk` and `sk`, then supply AWS
credentials and region through the SDK's normal chain:

```bash
mcp-v8 --http-port 8080 --heap-store none --fs-store s3 \
  --dynamodb-table mcp-metadata --dynamodb-namespace my-app \
  --s3-bucket my-mcp-blobs --cache-dir ./data/cache
```

`MCP_V8_DYNAMODB_TABLE`, `MCP_V8_DYNAMODB_NAMESPACE`, and equivalent config keys
select the same settings. Keep the namespace stable across replacements. Table
creation, IAM, key encoding, and endpoint integration tests are documented in the
[metadata crate guide](https://github.com/r33drichards/mcp-js/blob/main/server/metadata-store/README.md).
The [Lambda tutorial](../tutorials/lambda-dynamodb.md) covers a complete deployment.

Metadata obeys native limits, including DynamoDB's 400 KiB item limit with key
and attribute overhead. Oversized metadata is rejected; there is no automatic
128 KiB spillover to S3.

## Locate payloads and local state

Artifact and console bytes use `--s3-bucket` when configured, otherwise files
under `<session-db-path>/runtime-blobs`. Their database records hold metadata
and references. Changing metadata backends does not relocate these objects.

Sled execution metadata defaults to `<session-db-path>/executions-<http-port>`
for HTTP deployments, or `<session-db-path>/executions` without an HTTP port.
SQLite and DynamoDB avoid that per-port runtime namespace, but retain local
cancellation handles only for the process actually executing the code.

Preserve metadata and blob stores together. Selecting another backend does not
migrate old sled records. Runtime metadata has no automatic expiry, and filesystem
GC does not sweep runtime blobs. [Runtime storage](../concepts/runtime-storage.md)
describes the explicit local maintenance API.

## Cluster and scheduler boundaries

SQLite and DynamoDB selectors conflict with each other and with `--cluster-port`.
They do not replace the existing sled-backed Raft log, scheduler checkpoints, or
HTTP-function definition registry. An optional Rust Raft metadata adapter is
available for explicit integration; server bootstrap does not adopt it automatically.

## Source

- [Bootstrap and namespaces](https://github.com/r33drichards/mcp-js/blob/main/server/src/bootstrap.rs)
- [SQLite implementation](https://github.com/r33drichards/mcp-js/blob/main/server/metadata-store/src/sqlite.rs)
- [DynamoDB implementation](https://github.com/r33drichards/mcp-js/blob/main/server/metadata-store/src/dynamodb.rs)
