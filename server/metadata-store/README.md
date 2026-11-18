# Metadata backends

`MetadataStore` is the asynchronous, binary-safe persistence boundary for
mcp-js session histories, heap tags, and filesystem label heads/reflogs.
`SledStore` preserves existing sled tree names/key/value encodings and remains
the default. `SqliteStore` adds embedded SQLite and `DynamoDbStore` adds shared
DynamoDB metadata.

## SQLite configuration

```sh
mkdir -p ./data
mcp-v8 --http-port 3000 --sqlite-path ./data/metadata.sqlite --fs-store dir
```

`MCP_V8_SQLITE_PATH` or `sqlite_path` in a TOML/JSON config file selects the same
backend. The parent directory must exist; the database and schema are created
on first use by an enabled persistence axis (`--heap-store` or `--fs-store`).
Sessions, tags and labels use separate namespaces inside one database file.
An open/schema error aborts startup instead of falling back to sled.

SQLite uses a bundled library, WAL journaling, `synchronous=FULL`, and a five-second
busy timeout. Async operations run on Tokio's blocking pool. CAS reserves the
writer before comparing and commits the primary and companions in one transaction,
including across independent connections. Binary BLOB keys preserve byte ordering.
IDs are durable, namespace-scoped increasing positive integers up to `i64::MAX`;
exhaustion is an error. Successful writes are already durable, so `flush` is a no-op.

This is embedded, local SQLite, not rqlite. `--sqlite-path` conflicts with
`--dynamodb-table` and `--cluster-port`; it does not change the app's sled-backed
Raft log. Use a local filesystem that supports SQLite locking, and include the
WAL in live backups or use SQLite's backup API. No automatic migration from sled
is performed. The execution registry, console output, artifacts, scheduler and
HTTP-function definitions still use their existing stores. Snapshot blobs still
use the separately selected directory or S3 backend. FFI constructors retain sled
as their default.

```sh
cargo test -p mcp-metadata-store --test contract sqlite
cargo test -p server --test metadata_backends sqlite_consumers
cargo test -p server --test sqlite_integration --locked -- --test-threads=1
```

The real-server integration suite spawns the compiled server binary with isolated
local data. It executes JavaScript, persists heap references and restores session files after process
restart, reads heap tags through MCP, checks session histories and label reflogs
through HTTP, and verifies rejected label CAS writes no reflog entry. It also
checks CLI/environment/config selection against the actual SQLite rows and
invalid-path startup failure without sled fallback. These tests need neither
Docker nor an external database and run in the SQLite metadata CI workflow.

## Server configuration

Provision a DynamoDB table with **String** partition key `pk` and **String** sort
key `sk`. No indexes are required. The server never creates or changes tables.

```sh
aws dynamodb create-table --table-name mcp-metadata \
  --attribute-definitions AttributeName=pk,AttributeType=S AttributeName=sk,AttributeType=S \
  --key-schema AttributeName=pk,KeyType=HASH AttributeName=sk,KeyType=RANGE \
  --billing-mode PAY_PER_REQUEST

mcp-v8 --http-port 3000 --dynamodb-table mcp-metadata \
  --dynamodb-namespace pi --fs-store s3 --s3-bucket pi-files
```

The standard AWS SDK credential/region chain is used, including Lambda execution
roles. Do not embed AWS keys. `MCP_V8_DYNAMODB_TABLE` and
`MCP_V8_DYNAMODB_NAMESPACE` are the environment equivalents; flags also work in
`--config`. Endpoint overrides (`AWS_ENDPOINT_URL_DYNAMODB`) are useful for local
emulators. Leave `--cache-dir` unset for direct S3 snapshot storage without a
local file cache. S3 IAM permissions are separate from DynamoDB permissions.

Grant the role GetItem, PutItem, UpdateItem, Query, Scan and the PutItem actions
needed by TransactWriteItems on the table. DynamoDB evaluates transaction
permissions against the constituent operations (there is no separate
TransactWriteItems IAM action). No table creation/deletion privilege is needed
by the server. Namespace isolation is logical, not an authorization boundary.

DynamoDB cannot be combined with `--cluster-port`: it already arbitrates shared
metadata writes and is not a backend for the existing Raft log. Raft, scheduler
checkpoints and HTTP-function definitions are **not migrated** by this change.
Execution records, console pages, artifact records and upload grants use the
`/runtime` namespace; values larger than 128 KiB require S3 blob storage.
Completed records can be read by a fresh process, but running isolates and
cancellation handles remain process-local. This flag does not promise a completely
diskless runtime. FFI constructors keep their existing sled defaults.

For deployment steps, see the [Lambda tutorial](../../site-docs/tutorials/lambda-dynamodb.md).

## Semantics and limits

- Point reads and queries are strongly consistent. A multi-page traversal is not
  a transactionally frozen snapshot. Query pagination handles the 1 MiB boundary.
- Atomic conditional replacement plus companion writes powers label-head CAS
  with a reflog append in one transaction. A comparison miss returns false;
  service/permission/transport errors remain errors. No silent sled fallback.
- Binary keys use order-preserving hex encoding. Empty keys and NUL-containing
  prefixes are supported. Each logical tree maps to a partition, isolated by
  namespace; sessions, tags and labels use separate namespace suffixes.
- DynamoDB limits apply: 400 KiB/item, 4 MiB/transaction, 100 writes/transaction,
  2048-byte partition keys and 1024-byte encoded sort keys. Large code/history
  entries may need externalization to S3; this implementation rejects them.
- Session discovery scans the table with a namespace filter. Use a dedicated
  table where that cost matters. Prefix reads within a tree use Query.
- Shared counters allocate monotonic unique IDs (gaps are allowed). They and a
  busy tree can become hot partitions. There is no automatic migration of old
  sled data or cross-region/global-table consistency guarantee.

## Integration tests

The same contract exercises sled and a real DynamoDB-compatible service: atomic
head/reflog writes, rollback on conflict, concurrent CAS, binary prefix ordering,
reverse/limited queries, >1 MiB pagination, reopen and namespace isolation.

With Docker available:

```sh
cargo test -p mcp-metadata-store --features testcontainers-tests
```

Testcontainers starts a disposable pinned LocalStack image and cleans it up on
exit. All requests use fake credentials and an explicit loopback endpoint, never
your AWS account. The dedicated GitHub workflow runs this automatically.

For environments without nested Docker, start AWS DynamoDB Local and run:

```sh
DYNAMODB_TEST_ENDPOINT=http://127.0.0.1:8000 \
  cargo test -p mcp-metadata-store --test contract dynamodb_endpoint_contract -- --ignored
```

The backend test crate does not link V8. It follows the disposable-emulator
approach described in [AWS Builder's LocalStack/Testcontainers guide](https://builder.aws.com/content/2dxWQAZsdc3dk5uCILAmNqEME2e/testing-dynamodb-interactions-using-localstack-and-testcontainers).

### Heap restoration regression

The real-server suite also restores V8 heaps with both SQLite and default sled
metadata. On Unix, each backend crosses two graceful process restarts, uses an explicit heap handle
and automatic session resolution, checks accumulated JavaScript state and mounted
file contents, and verifies the recorded input/output heap chain. Generated entry
module URLs must remain unique across processes because their module registrations
are retained in the heap snapshot. The CI command runs this entire test binary,
including the sled regression.
