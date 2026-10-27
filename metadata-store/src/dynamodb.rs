use crate::{MetadataStore, Result, Row, Write};
use async_trait::async_trait;
use aws_sdk_dynamodb::{
    Client,
    primitives::Blob,
    types::{AttributeValue as A, Put, TransactWriteItem},
};
use std::collections::{BTreeSet, HashMap};

/// A pre-provisioned table with String partition key `pk` and String sort key
/// `sk`. Each namespace is isolated; each tree occupies one partition. Hex
/// encoding preserves binary-key ordering and supports empty keys via a sentinel.
#[derive(Clone)]
pub struct DynamoDbStore {
    client: Client,
    table: String,
    prefix: String,
}
impl DynamoDbStore {
    pub fn new(client: Client, table: impl Into<String>, namespace: &str) -> Result<Self> {
        let table = table.into();
        if table.is_empty() || namespace.is_empty() || namespace.len() > 128 {
            return Err("table and namespace are required (namespace max 128 bytes)".into());
        }
        Ok(Self {
            client,
            table,
            prefix: format!("{}:", hex::encode(namespace)),
        })
    }
    fn pk(&self, tree: &str) -> String {
        format!("{}{}", self.prefix, hex::encode(tree))
    }
    fn sk(key: &[u8]) -> String {
        format!("k{}", hex::encode(key))
    }
    fn keys(&self, tree: &str, key: &[u8]) -> HashMap<String, A> {
        HashMap::from([
            ("pk".into(), A::S(self.pk(tree))),
            ("sk".into(), A::S(Self::sk(key))),
        ])
    }
    fn item(&self, w: &Write) -> Result<HashMap<String, A>> {
        let pk = self.pk(&w.tree);
        let sk = Self::sk(&w.key);
        // Conservative size bound, including attribute names and headroom.
        if pk.len() > 2048
            || sk.len() > 1024
            || pk.len() + sk.len() + w.value.len() + 64 > 400 * 1024
        {
            return Err(
                "DynamoDB metadata key/item size limit exceeded; put payloads in S3".into(),
            );
        }
        let mut item = self.keys(&w.tree, &w.key);
        item.insert("v".into(), A::B(Blob::new(w.value.clone())));
        Ok(item)
    }
    fn value(item: &HashMap<String, A>) -> Result<Vec<u8>> {
        item.get("v")
            .and_then(|v| v.as_b().ok())
            .map(|b| b.as_ref().to_vec())
            .ok_or_else(|| "invalid metadata value".into())
    }
}

#[async_trait]
impl MetadataStore for DynamoDbStore {
    async fn get(&self, tree: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let result = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(self.keys(tree, key)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| format!("DynamoDB get: {e:?}"))?;
        result.item.as_ref().map(Self::value).transpose()
    }
    async fn put(&self, write: Write) -> Result<()> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(self.item(&write)?))
            .send()
            .await
            .map_err(|e| format!("DynamoDB put: {e:?}"))?;
        Ok(())
    }
    async fn scan(
        &self,
        tree: &str,
        prefix: &[u8],
        reverse: bool,
        limit: Option<usize>,
    ) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        let max = limit.unwrap_or(usize::MAX);
        let mut cursor = None;
        while out.len() < max {
            let response = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("pk = :p AND begins_with(sk, :s)")
                .expression_attribute_values(":p", A::S(self.pk(tree)))
                .expression_attribute_values(":s", A::S(Self::sk(prefix)))
                .consistent_read(true)
                .scan_index_forward(!reverse)
                .limit((max - out.len()).min(1000) as i32)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|e| format!("DynamoDB query: {e:?}"))?;
            for item in response.items() {
                let key = item
                    .get("sk")
                    .and_then(|v| v.as_s().ok())
                    .and_then(|v| v.strip_prefix('k'))
                    .ok_or("invalid metadata key")?;
                out.push((
                    hex::decode(key).map_err(|e| e.to_string())?,
                    Self::value(item)?,
                ));
            }
            cursor = response.last_evaluated_key;
            if cursor.as_ref().is_none_or(|k| k.is_empty()) {
                break;
            }
        }
        Ok(out)
    }
    async fn compare_and_swap(
        &self,
        primary: Write,
        expected: Option<Vec<u8>>,
        companions: Vec<Write>,
    ) -> Result<bool> {
        if companions.len() >= 100 {
            return Err("DynamoDB transaction exceeds 100 items".into());
        }
        let writes: Vec<_> = std::iter::once(primary).chain(companions).collect();
        let mut seen = BTreeSet::new();
        let mut actions = Vec::new();
        let mut total = 0;
        for (i, w) in writes.iter().enumerate() {
            if !seen.insert((&w.tree, &w.key)) {
                return Err("duplicate transaction key".into());
            }
            let item = self.item(w)?;
            total += self.pk(&w.tree).len() + Self::sk(&w.key).len() + w.value.len() + 64;
            let mut put = Put::builder().table_name(&self.table).set_item(Some(item));
            if i == 0 {
                put = if let Some(value) = &expected {
                    put.condition_expression("#v = :expected")
                        .expression_attribute_names("#v", "v")
                        .expression_attribute_values(":expected", A::B(Blob::new(value.clone())))
                } else {
                    put.condition_expression("attribute_not_exists(pk)")
                };
            }
            actions.push(
                TransactWriteItem::builder()
                    .put(put.build().map_err(|e| e.to_string())?)
                    .build(),
            );
        }
        if total > 4 * 1024 * 1024 {
            return Err("DynamoDB transaction exceeds 4 MiB".into());
        }
        // TransactionConflict is not a failed comparison. Retry only that
        // definite no-commit result; never replay an ambiguous transport error.
        let request_token = uuid::Uuid::new_v4().to_string();
        for attempt in 0..8 {
            match self
                .client
                .transact_write_items()
                .client_request_token(&request_token)
                .set_transact_items(Some(actions.clone()))
                .send()
                .await
            {
                Ok(_) => return Ok(true),
                Err(e) => {
                    if let Some(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(c)) = e.as_service_error() {
                        let reasons = c.cancellation_reasons();
                        if reasons.first().and_then(|r| r.code()) == Some("ConditionalCheckFailed") && reasons.iter().skip(1).all(|r| r.code().is_none_or(|c| c == "None")) { return Ok(false); }
                        if reasons.iter().any(|r| r.code() == Some("TransactionConflict")) && reasons.iter().all(|r| r.code().is_none_or(|c| c == "None" || c == "TransactionConflict")) && attempt < 7 {
                            tokio::time::sleep(std::time::Duration::from_millis(5 << attempt)).await;
                            continue;
                        }
                    }
                    return Err(format!("DynamoDB transaction: {e:?}"));
                }
            }
        }
        unreachable!()
    }
    async fn generate_id(&self) -> Result<u64> {
        // Atomic increment; gaps are allowed, uniqueness and monotonicity are not.
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(HashMap::from([
                ("pk".into(), A::S(format!("{}!counter", self.prefix))),
                ("sk".into(), A::S("id".into())),
            ])))
            .update_expression("ADD #n :one")
            .expression_attribute_names("#n", "n")
            .expression_attribute_values(":one", A::N("1".into()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::UpdatedNew)
            .send()
            .await
            .map_err(|e| format!("DynamoDB sequence: {e:?}"))?;
        result
            .attributes
            .as_ref()
            .and_then(|a| a.get("n"))
            .and_then(|v| v.as_n().ok())
            .ok_or("missing sequence")?
            .parse()
            .map_err(|e| format!("invalid sequence: {e}"))
    }
    async fn tree_names(&self) -> Result<Vec<String>> {
        // Session discovery is uncommon. Scan is paginated and namespace-
        // filtered; no GSI is required. Reads within a tree use Query instead.
        let mut names = BTreeSet::new();
        let mut cursor = None;
        loop {
            let result = self
                .client
                .scan()
                .table_name(&self.table)
                .consistent_read(true)
                .projection_expression("pk")
                .filter_expression("begins_with(pk, :p)")
                .expression_attribute_values(":p", A::S(self.prefix.clone()))
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|e| format!("DynamoDB trees: {e:?}"))?;
            for item in result.items() {
                let pk = item
                    .get("pk")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("invalid partition key")?;
                if let Some(encoded) = pk.strip_prefix(&self.prefix) {
                    if encoded == "!counter" {
                        continue;
                    }
                    let tree = String::from_utf8(hex::decode(encoded).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                    names.insert(tree);
                }
            }
            cursor = result.last_evaluated_key;
            if cursor.as_ref().is_none_or(|k| k.is_empty()) {
                break;
            }
        }
        Ok(names.into_iter().collect())
    }
    fn flush(&self) -> Result<()> {
        Ok(())
    }
}
