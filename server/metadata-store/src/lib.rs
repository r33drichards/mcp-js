//! Async, binary-safe metadata persistence. Snapshot payloads belong in S3,
//! not this store. A failed compare must leave all companion writes untouched.
use async_trait::async_trait;
pub mod dynamodb;
pub mod sled_store;
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
    /// DynamoDB writes are durable on acknowledgement. Sled flushes locally.
    fn flush(&self) -> Result<()>;
}
