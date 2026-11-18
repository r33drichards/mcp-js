//! Async, binary-safe metadata persistence. Snapshot payloads use separate blob
//! storage. A failed compare must leave all companion writes untouched.
use async_trait::async_trait;
#[cfg(feature="dynamodb")]
pub mod dynamodb;
pub mod sled_store;
pub mod sqlite;
pub use sqlite::SqliteStore;
#[cfg(feature="dynamodb")]
pub use dynamodb::DynamoDbStore;
pub use sled_store::SledStore;
pub type Result<T> = std::result::Result<T, String>;
pub type Row = (Vec<u8>, Vec<u8>);

#[derive(Clone, Debug)]
pub struct Write {
    pub tree: String,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}
impl Write {
    pub fn new(tree: &str, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            tree: tree.into(),
            key: key.into(),
            value: value.into(),
        }
    }
}

#[async_trait]
pub trait MetadataStore: Send + Sync {
    /// Standalone backends provide current point reads; Raft adapters must
    /// explicitly advertise weaker local-committed reads until a barrier exists.
    fn supports_linearizable_reads(&self) -> bool { true }
    /// A Raft mismatch against pending state is not a committed ownership observation.
    fn provisional_conflicts(&self) -> bool { false }
    /// Pure validation before proposal/transport. Known size/shape rejections
    /// are errors, distinct from an uncertain acknowledgement after mutation.
    fn validate_transaction(&self, _primary: &Write, _expected: Option<&[u8]>, _companions: &[Write]) -> Result<()> { Ok(()) }

    async fn get(&self, tree: &str, key: &[u8]) -> Result<Option<Vec<u8>>>;
    async fn put(&self, write: Write) -> Result<()>;
    /// Byte-lexicographic order, optionally reversed. Limit applies after prefix
    /// selection. Results are not a multi-page point-in-time snapshot.
    async fn scan(
        &self,
        tree: &str,
        prefix: &[u8],
        reverse: bool,
        limit: Option<usize>,
    ) -> Result<Vec<Row>>;
    /// Atomically compare the primary value (None means absent), replace it,
    /// and apply companions, even across trees. False means comparison failed;
    /// transport/permission/storage errors must never be reported as false.
    async fn compare_and_swap(
        &self,
        primary: Write,
        expected: Option<Vec<u8>>,
        companions: Vec<Write>,
    ) -> Result<bool>;
    async fn generate_id(&self) -> Result<u64>;
    async fn tree_names(&self) -> Result<Vec<String>>;
    /// DynamoDB and SQLite writes are durable on acknowledgement. Sled flushes locally.
    fn flush(&self) -> Result<()>;
}

pub mod runtime_tree;
pub use runtime_tree::{RuntimeStore, RuntimeTree};

pub mod blob;

pub mod content;

pub mod publication;
