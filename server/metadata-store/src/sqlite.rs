//! Embedded SQLite metadata. Consensus remains the responsibility of the caller.
use crate::{MetadataStore, Result, Row, Write};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
    namespace: String,
}

impl SqliteStore {
    /// Open a local database file. Its parent directory must already exist.
    /// Namespaces isolate sessions, tags, labels and runtime records in one file.
    pub fn open(path: impl AsRef<Path>, namespace: &str) -> Result<Self> {
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS metadata (
                 namespace TEXT NOT NULL, tree TEXT NOT NULL,
                 key BLOB NOT NULL, value BLOB NOT NULL,
                 PRIMARY KEY(namespace, tree, key)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS metadata_ids (
                 namespace TEXT PRIMARY KEY, value INTEGER NOT NULL
             ) WITHOUT ROWID;",
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            namespace: namespace.into(),
        })
    }

    // SQLite may wait on a writer or fsync. Keep that work off Tokio's executor.
    async fn run<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection, &str) -> rusqlite::Result<T> + Send + 'static,
    {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = store.connection.lock().map_err(|e| e.to_string())?;
            operation(&mut connection, &store.namespace).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

fn put(connection: &Connection, namespace: &str, w: &Write) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO metadata(namespace, tree, key, value) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(namespace, tree, key) DO UPDATE SET value=excluded.value",
        params![namespace, w.tree, w.key, w.value],
    )?;
    Ok(())
}

#[async_trait]
impl MetadataStore for SqliteStore {
    fn runtime_inline_limit(&self) -> Option<usize> {
        None
    }

    async fn get(&self, tree: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let (tree, key) = (tree.to_owned(), key.to_vec());
        self.run(move |connection, namespace| {
            connection
                .query_row(
                    "SELECT value FROM metadata WHERE namespace=?1 AND tree=?2 AND key=?3",
                    params![namespace, tree, key],
                    |row| row.get(0),
                )
                .optional()
        })
        .await
    }

    async fn put(&self, write: Write) -> Result<()> {
        self.run(move |connection, namespace| put(connection, namespace, &write))
            .await
    }

    async fn scan(
        &self,
        tree: &str,
        prefix: &[u8],
        reverse: bool,
        limit: Option<usize>,
    ) -> Result<Vec<Row>> {
        let (tree, prefix) = (tree.to_owned(), prefix.to_vec());
        self.run(move |connection, namespace| {
            // A prefix is a byte range. There is no finite upper bound for an
            // empty or all-0xff prefix; the lower bound suffices in those cases.
            let mut upper = prefix.clone();
            while upper.last() == Some(&255) {
                upper.pop();
            }
            let upper = upper
                .last_mut()
                .map(|last| {
                    *last += 1;
                })
                .map(|_| upper);
            let order = if reverse { "DESC" } else { "ASC" };
            let sql = format!(
                "SELECT key, value FROM metadata
                 WHERE namespace=?1 AND tree=?2 AND key>=?3
                   AND (?4 IS NULL OR key<?4) ORDER BY key {order} LIMIT ?5"
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(
                params![
                    namespace,
                    tree,
                    prefix,
                    upper,
                    limit.map(|n| n.min(i64::MAX as usize) as i64).unwrap_or(-1)
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            rows.collect()
        })
        .await
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
        self.run(move |connection, namespace| {
            // Acquire the write reservation before reading, including across
            // independent connections/processes. A comparison miss writes nothing.
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let actual: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT value FROM metadata WHERE namespace=?1 AND tree=?2 AND key=?3",
                    params![namespace, writes[0].tree, writes[0].key],
                    |row| row.get(0),
                )
                .optional()?;
            if actual != expected {
                return Ok(false);
            }
            for write in &writes {
                put(&transaction, namespace, write)?;
            }
            transaction.commit()?;
            Ok(true)
        })
        .await
    }

    async fn generate_id(&self) -> Result<u64> {
        self.run(|connection, namespace| {
            let value: i64 = connection.query_row(
                "INSERT INTO metadata_ids(namespace, value) VALUES (?1, 1)
                 ON CONFLICT(namespace) DO UPDATE SET value=value+1
                 WHERE value<9223372036854775807 RETURNING value",
                [namespace],
                |row| row.get(0),
            )?;
            Ok(value as u64)
        })
        .await
    }

    async fn tree_names(&self) -> Result<Vec<String>> {
        self.run(|connection, namespace| {
            let mut statement = connection
                .prepare("SELECT DISTINCT tree FROM metadata WHERE namespace=?1 ORDER BY tree")?;
            statement
                .query_map([namespace], |row| row.get(0))?
                .collect()
        })
        .await
    }

    fn flush(&self) -> Result<()> {
        // Every successful write already committed with synchronous=FULL.
        // WAL checkpointing is not required for durability.
        Ok(())
    }
}
