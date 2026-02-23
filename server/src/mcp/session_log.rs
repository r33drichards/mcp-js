use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLogEntry {
    pub input_heap: Option<String>,
    pub output_heap: String,
    pub code: String,
    pub timestamp: String, // ISO 8601 UTC
}

#[derive(Clone)]
pub struct SessionLog {
    db: sled::Db,
}

#[allow(dead_code)]
impl SessionLog {
    pub fn new(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self { db })
    }

    pub fn from_config(config: sled::Config) -> Result<Self, String> {
        let db = config.open().map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self { db })
    }

    /// Append a log entry to the given session. Returns the sequence number.
    pub fn append(&self, session: &str, entry: SessionLogEntry) -> Result<u64, String> {
        let tree = self
            .db
            .open_tree(session)
            .map_err(|e| format!("Failed to open tree '{}': {}", session, e))?;

        let value =
            serde_json::to_vec(&entry).map_err(|e| format!("Failed to serialize entry: {}", e))?;

        // generate_id() returns a monotonically increasing u64, unique across the Db
        let seq = self
            .db
            .generate_id()
            .map_err(|e| format!("Failed to generate id: {}", e))?;

        let key = seq.to_be_bytes();
        tree.insert(key, value)
            .map_err(|e| format!("Failed to insert entry: {}", e))?;

        Ok(seq)
    }

    /// List all session names.
    pub fn list_sessions(&self) -> Result<Vec<String>, String> {
        let names: Vec<String> = self
            .db
            .tree_names()
            .into_iter()
            .filter_map(|name| {
                let s = String::from_utf8(name.to_vec()).ok()?;
                // Filter out sled's default tree
                if s == "__sled__default" {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();
        Ok(names)
    }

    /// List all entries for a session, optionally filtering to specific fields.
    /// Valid field names: "index", "input_heap", "output_heap", "code", "timestamp".
    /// If fields is None, all fields are returned.
    pub fn list_entries(
        &self,
        session: &str,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let tree = self
            .db
            .open_tree(session)
            .map_err(|e| format!("Failed to open tree '{}': {}", session, e))?;

        let mut results = Vec::new();

        for item in tree.iter() {
            let (key_bytes, val_bytes) =
                item.map_err(|e| format!("Failed to read entry: {}", e))?;

            let index = u64::from_be_bytes(
                key_bytes
                    .as_ref()
                    .try_into()
                    .map_err(|_| "Invalid key length".to_string())?,
            );

            let entry: SessionLogEntry = serde_json::from_slice(&val_bytes)
                .map_err(|e| format!("Failed to deserialize entry: {}", e))?;

            let value = match &fields {
                Some(field_list) => {
                    let mut obj = serde_json::Map::new();
                    for field in field_list {
                        match field.as_str() {
                            "index" => {
                                obj.insert(
                                    "index".to_string(),
                                    serde_json::Value::Number(index.into()),
                                );
                            }
                            "input_heap" => {
                                obj.insert(
                                    "input_heap".to_string(),
                                    match &entry.input_heap {
                                        Some(h) => serde_json::Value::String(h.clone()),
                                        None => serde_json::Value::Null,
                                    },
                                );
                            }
                            "output_heap" => {
                                obj.insert(
                                    "output_heap".to_string(),
                                    serde_json::Value::String(entry.output_heap.clone()),
                                );
                            }
                            "code" => {
                                obj.insert(
                                    "code".to_string(),
                                    serde_json::Value::String(entry.code.clone()),
                                );
                            }
                            "timestamp" => {
                                obj.insert(
                                    "timestamp".to_string(),
                                    serde_json::Value::String(entry.timestamp.clone()),
                                );
                            }
                            _ => {} // ignore unknown fields
                        }
                    }
                    serde_json::Value::Object(obj)
                }
                None => {
                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        "index".to_string(),
                        serde_json::Value::Number(index.into()),
                    );
                    obj.insert(
                        "input_heap".to_string(),
                        match &entry.input_heap {
                            Some(h) => serde_json::Value::String(h.clone()),
                            None => serde_json::Value::Null,
                        },
                    );
                    obj.insert(
                        "output_heap".to_string(),
                        serde_json::Value::String(entry.output_heap.clone()),
                    );
                    obj.insert(
                        "code".to_string(),
                        serde_json::Value::String(entry.code.clone()),
                    );
                    obj.insert(
                        "timestamp".to_string(),
                        serde_json::Value::String(entry.timestamp.clone()),
                    );
                    serde_json::Value::Object(obj)
                }
            };

            results.push(value);
        }

        Ok(results)
    }

    /// Get the latest entry for a session, if any.
    pub fn get_latest(&self, session: &str) -> Result<Option<SessionLogEntry>, String> {
        let tree = self
            .db
            .open_tree(session)
            .map_err(|e| format!("Failed to open tree '{}': {}", session, e))?;

        match tree.last() {
            Ok(Some((_key, val))) => {
                let entry: SessionLogEntry = serde_json::from_slice(&val)
                    .map_err(|e| format!("Failed to deserialize entry: {}", e))?;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Failed to get latest entry: {}", e)),
        }
    }

    /// Flush all pending writes to disk.
    pub fn flush(&self) -> Result<(), String> {
        self.db
            .flush()
            .map_err(|e| format!("Failed to flush sled db: {}", e))?;
        Ok(())
    }
}
