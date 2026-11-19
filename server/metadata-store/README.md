# Metadata backends

`MetadataStore` is the asynchronous, binary-safe persistence boundary for
mcp-js session histories, heap tags, filesystem label heads/reflogs and runtime
bookkeeping (execution records, console pages, artifacts and upload grants).
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
at startup. Sessions, tags, labels and runtime bookkeeping use separate namespaces
inside one database file. Runtime records use the `runtime` namespace, independent
of the HTTP port, and survive process replacement. Runtime persistence also works
without heap or filesystem snapshot persistence. Default sled runtime metadata
uses the execution database under the configured session database directory.
An open/schema error aborts startup instead of falling back to sled.

The server stores artifact and console payloads outside the database, including
empty artifacts and small console pages. Configure `--s3-bucket` (or
`MCP_V8_S3_BUCKET`) to use S3; otherwise payloads use files under
`<session-db-path>/runtime-blobs`. This applies with SQLite, DynamoDB or default
sled metadata. The database reference is published only after the blob write
succeeds. Overwrites publish a new immutable object reference, and sled references
are flushed before acknowledgement.

Both backends reuse the heap `HeapStorage` interface: `S3HeapStorage` and
`FileHeapStorage`. Objects use the `runtime/` prefix (S3 needs `s3:PutObject` and
`s3:GetObject` permissions there). Backend selection happens at startup: S3
errors never switch to filesystem storage, and filesystem errors never store the
payload inline. Heap/filesystem snapshot selection remains independent.

Old inline records and legacy S3 references remain readable. New artifact/console
writes always use the selected blob backend. Changing that backend requires
moving existing objects; no read-error fallback or automatic migration is
performed. Processes sharing metadata must also share the same S3 bucket or
filesystem payload directory. Overwrites and failed reference publication may
leave unreferenced objects. Local managed roots offer an explicit reader-safe
collector; it requires all registered metadata owners and has no automatic timer.
See [runtime blob maintenance](../../docs/runtime-blob-maintenance.md). S3 and
cross-domain reclamation remain unsupported. New content-addressed references use `runtime/v2/` keys;
include that prefix in existing `runtime/` object permissions.

Execution records and upload grants remain database values. There is no 128 KiB
spill threshold: the selected database enforces its native limits, and oversized
metadata writes fail rather than being externalized to S3. SQLite can therefore
keep large execution results, while DynamoDB rejects items above its 400 KiB
limit (including keys and attributes).

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
is performed, including existing console, artifact and upload-grant data in sled.
Scheduler and HTTP-function definitions still use their existing stores.
Snapshot blobs still use the separately selected directory or S3 backend. FFI constructors retain sled
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

Runtime coverage checks all three selection methods across hard process restarts:
large execution results, paginated console output, artifacts, grant redemption,
one-time use across independent server processes, and rejection of reuse after
another restart. Tests use a disposable S3 HTTP fixture through the real AWS SDK,
inspect SQLite references and verify no sled execution database was created.
Coverage also checks small payloads with sled, filesystem persistence across
hard restarts, failed S3 uploads preserving the previous reference, missing
objects without fallback, and filesystem errors without database fallback.

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
`/runtime` namespace. Artifact and console bytes use the configured S3 bucket or
the local payload directory; execution records and upload grants stay in DynamoDB
and obey its native item limits.
Completed records can be read by a fresh process, but running isolates and
cancellation handles remain process-local. This flag does not promise a completely
diskless runtime. FFI constructors keep their existing sled defaults.

For deployment steps, see the [Lambda tutorial](../../site-docs/tutorials/lambda-dynamodb.md).

## Shared object and publication APIs

`blob::BlobStorage` is the shared named-blob boundary (`HeapStorage` and
`RuntimeBlobs` remain compatibility names). Immutable writes must acknowledge an
identical existing object or fail on conflicting content; they never overwrite
it. Server memory, filesystem and S3 backends implement the contract. Common
backend construction also gives runtime S3 storage the existing cache support.

`content::ContentObjects` stores versioned SHA-256 objects and verifies size and
digest when reading. New console pages hold compact typed references. Artifact
metadata stays in the database and points to payload-only objects, so timestamps
and MIME types do not change payload identity; listing metadata needs no blob
fetch. New descriptors live in a separate `runtime_refs:v1:<hex tree name>`
index; arbitrary legacy inline bytes are never interpreted using a magic prefix.
Inline records and explicit old DynamoDB S3 envelope references remain readable;
legacy object names must be exactly `runtime/<canonical UUID>`. Earlier unmerged
marker-only PR formats are ambiguous and need an explicit migration if used. Heap/fs
snapshot hashes and formats are unchanged. Compatibility is one-way: old binaries
do not understand the new runtime reference format. Shared deployments must
pause runtime writes while upgrading their readers. Rolling back requires
restoring compatible metadata/object formats.
Direct execution registries initialize
filesystem payload storage by default; explicit raw-tree APIs and `from_store`
without an attached blob backend retain inline behavior. Console objects use
256 KiB pages and indexed lengths/newline counts for lazy byte/line windows.
Blob work has bounded concurrency and a queue-inclusive deadline, independently
of the metadata reactor.

`publication::MetadataPublication` is an opt-in revisioned metadata facade.
Publication commits the primary, operation receipt and index companions atomically
and flushes before acknowledgement. Retries use a per-key operation ID, including
a request fingerprint; a reused ID with different input fails. Tombstones retain
revisions. Preflight validation returns known backend size/shape errors before
mutation; transport/flush uncertainty after mutation returns `UnknownOutcome`.
Receipts have no automatic expiry in this initial API.

`publication::ObjectPublication` composes immutable object storage with the
publication facade, writing the object before metadata. The optional server
`raft_metadata::RaftMetadataStore` adapts binary metadata to consensus. Its reads
are local committed reads; stronger reads are rejected and pending CAS conflicts
remain explicitly provisional. It is exercised by the V8-free consensus harness,
not implicitly enabled for existing server domains. No SQLite Raft persistence
or automatic blob replication is introduced. Cross-domain GC and migration of
existing domain routing are separate work.

The metadata crate's default `dynamodb` feature preserves the existing SDK-backed
backend. Consensus-only consumers can disable default features to avoid compiling
that SDK. See the [content-addressed storage design](../../docs/content-addressed-storage-design.md)
for the boundaries, migration stages and remaining guarantees.

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

### Publication example (Rust)

```rust,ignore
use mcp_metadata_store::{content::{ContentObjects, ObjectKind}, publication::{
    MetadataPublication, ObjectPublication, Publication, PublishOutcome,
}};

// metadata: Arc<dyn MetadataStore>, e.g. SqliteStore or RaftMetadataStore.
// blobs: Arc<dyn BlobStorage>, e.g. FileHeapStorage or S3HeapStorage.
let service = ObjectPublication {
    objects: ContentObjects::new(blobs),
    metadata: MetadataPublication::new(metadata, "artifacts")?,
};
let outcome = service.publish(Publication {
    operation_id: request_id, // retain this ID and the same request when retrying
    key: b"report.csv".to_vec(),
    expected_revision: None, // create; updates supply the observed revision
    value: None, // ObjectPublication fills in the typed content reference
    companions: vec![],
}, ObjectKind::Artifact, &bytes).await?;
match outcome {
    PublishOutcome::Committed { revision } => { /* acknowledge revision */ }
    PublishOutcome::UnknownOutcome { .. } => { /* retry same operation ID */ }
    PublishOutcome::ProvisionalConflict => { /* pending Raft mismatch; no ownership claim */ }
    PublishOutcome::Conflict => { /* standalone CAS conflict; choose next operation */ }
}
```

References returned by `MetadataPublication::read` use `ObjectRef::decode` on the
record value, followed by `ContentObjects::get` for verified bytes. A tombstone
retains its revision but has no value. An operation ID is scoped to its key and
publication namespace; receipts are not an authorization boundary or a garbage
collection policy.
