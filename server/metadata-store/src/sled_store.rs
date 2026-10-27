use crate::{MetadataStore, Result, Row, Write};
use async_trait::async_trait;
use sled::transaction::{ConflictableTransactionError, TransactionError, Transactional};

#[derive(Clone)]
pub struct SledStore(pub sled::Db);

#[async_trait]
impl MetadataStore for SledStore {
    async fn get(&self, tree: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.0
            .open_tree(tree)
            .and_then(|t| t.get(key))
            .map(|v| v.map(|v| v.to_vec()))
            .map_err(|e| e.to_string())
    }
    async fn put(&self, w: Write) -> Result<()> {
        self.0
            .open_tree(w.tree)
            .and_then(|t| t.insert(w.key, w.value))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    async fn scan(
        &self,
        tree: &str,
        prefix: &[u8],
        reverse: bool,
        limit: Option<usize>,
    ) -> Result<Vec<Row>> {
        let tree = self.0.open_tree(tree).map_err(|e| e.to_string())?;
        let iter = tree.scan_prefix(prefix);
        let iter: Box<dyn Iterator<Item = sled::Result<(sled::IVec, sled::IVec)>> + Send> =
            if reverse {
                Box::new(iter.rev())
            } else {
                Box::new(iter)
            };
        iter.take(limit.unwrap_or(usize::MAX))
            .map(|v| {
                v.map(|(k, v)| (k.to_vec(), v.to_vec()))
                    .map_err(|e| e.to_string())
            })
            .collect()
    }
    async fn compare_and_swap(
        &self,
        primary: Write,
        expected: Option<Vec<u8>>,
        companions: Vec<Write>,
    ) -> Result<bool> {
        let writes: Vec<_> = std::iter::once(primary).chain(companions).collect();
        let mut seen = std::collections::BTreeSet::new();
        if writes.iter().any(|w| !seen.insert((&w.tree, &w.key))) {
            return Err("duplicate transaction key".into());
        }
        let trees = writes
            .iter()
            .map(|w| self.0.open_tree(&w.tree))
            .collect::<sled::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let result = trees.as_slice().transaction(|tx| {
            let actual = tx[0].get(writes[0].key.as_slice())?;
            if actual.as_deref() != expected.as_deref() {
                return Err(ConflictableTransactionError::Abort(()));
            }
            for (tree, w) in tx.iter().zip(&writes) {
                tree.insert(w.key.as_slice(), w.value.as_slice())?;
            }
            Ok(())
        });
        match result {
            Ok(()) => Ok(true),
            Err(TransactionError::Abort(())) => Ok(false),
            Err(TransactionError::Storage(e)) => Err(e.to_string()),
        }
    }
    async fn generate_id(&self) -> Result<u64> {
        self.0.generate_id().map_err(|e| e.to_string())
    }
    async fn tree_names(&self) -> Result<Vec<String>> {
        self.0
            .tree_names()
            .into_iter()
            .filter(|n| n.as_ref() != b"__sled__default")
            .map(|n| String::from_utf8(n.to_vec()).map_err(|e| e.to_string()))
            .collect()
    }
    fn flush(&self) -> Result<()> {
        self.0.flush().map(|_| ()).map_err(|e| e.to_string())
    }
}
