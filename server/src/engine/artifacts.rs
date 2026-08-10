//! Keyed artifact store — lets sandboxed JS hand results of any content type
//! back to the MCP client: images, audio, CSVs, JSON, arbitrary binary.
//!
//! JS calls `artifact(key, mime, bytes)` to store (or overwrite) an artifact
//! under a caller-chosen key. Artifacts land in a sled tree (`"artifacts"`)
//! in the execution db, so they survive across executions and can be fetched
//! later with the `get_artifact` MCP tool. The MCP layer renders each
//! artifact as the closest MCP-spec content block for its mime type —
//! `image/*` → `ImageContent` and `audio/*` → `AudioContent` (base64 data +
//! mimeType, the spec's way to put images/audio in front of a model), UTF-8
//! payloads → `TextContent`, and other binary → base64 text.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

// ── Limits ───────────────────────────────────────────────────────────────

/// Maximum payload size for a single artifact. Generous for model-bound
/// images (Claude caps images at ~5 MB) while still bounding sled growth.
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

/// Maximum artifact key length in bytes.
pub const MAX_KEY_BYTES: usize = 256;

/// Maximum mime-type length in bytes.
pub const MAX_MIME_BYTES: usize = 128;

// ── Types ────────────────────────────────────────────────────────────────

/// Artifact metadata (everything except the payload bytes).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ArtifactMeta {
    pub key: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub created_at: String,
    /// Execution that (last) wrote this artifact, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
}

/// A stored artifact: metadata plus payload bytes.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub meta: ArtifactMeta,
    pub bytes: Vec<u8>,
}

/// Transport-agnostic rendering of an artifact payload, keyed off the stored
/// mime type. Artifacts are generic containers — any mime type, any bytes —
/// and the MCP handlers map each variant to the closest MCP-spec content
/// block: `Image` → `ImageContent`, `Audio` → `AudioContent` (both base64
/// data + mimeType per the spec), everything else → `TextContent`.
#[derive(Debug, Clone)]
pub enum ArtifactContent {
    /// `image/*` artifact → MCP `ImageContent` (base64 data + mime type).
    Image { data_base64: String, mime_type: String },
    /// `audio/*` artifact → MCP `AudioContent` (base64 data + mime type).
    Audio { data_base64: String, mime_type: String },
    /// UTF-8 payload of any other mime type → MCP `TextContent` (raw text).
    Text(String),
    /// Any other binary payload → MCP `TextContent` carrying base64.
    Base64(String),
}

impl Artifact {
    /// Render the payload for an MCP tool result.
    pub fn content(&self) -> ArtifactContent {
        if self.meta.mime_type.starts_with("image/") {
            ArtifactContent::Image {
                data_base64: BASE64.encode(&self.bytes),
                mime_type: self.meta.mime_type.clone(),
            }
        } else if self.meta.mime_type.starts_with("audio/") {
            ArtifactContent::Audio {
                data_base64: BASE64.encode(&self.bytes),
                mime_type: self.meta.mime_type.clone(),
            }
        } else {
            match std::str::from_utf8(&self.bytes) {
                Ok(s) => ArtifactContent::Text(s.to_string()),
                Err(_) => ArtifactContent::Base64(BASE64.encode(&self.bytes)),
            }
        }
    }

    /// How `content()` encodes the payload — surfaced in JSON metadata so
    /// callers know what they're getting.
    pub fn encoding(&self) -> &'static str {
        match self.content() {
            ArtifactContent::Image { .. } => "image",
            ArtifactContent::Audio { .. } => "audio",
            ArtifactContent::Text(_) => "utf-8",
            ArtifactContent::Base64(_) => "base64",
        }
    }
}

// ── Store ────────────────────────────────────────────────────────────────

/// Sled-backed keyed artifact store. Values are encoded as
/// `[u32 BE header_len][JSON header][payload bytes]` where the header carries
/// the metadata (mime type, timestamp, writing execution).
#[derive(Clone)]
pub struct ArtifactStore {
    tree: sled::Tree,
}

#[derive(Serialize, serde::Deserialize)]
struct StoredHeader {
    mime_type: String,
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_id: Option<String>,
}

impl ArtifactStore {
    pub fn new(tree: sled::Tree) -> Self {
        Self { tree }
    }

    /// Validate and store an artifact, overwriting any previous value under
    /// the same key. Returns the stored metadata.
    pub fn put(
        &self,
        key: &str,
        mime_type: &str,
        bytes: &[u8],
        execution_id: Option<&str>,
    ) -> Result<ArtifactMeta, String> {
        if key.is_empty() {
            return Err("artifact: key must be a non-empty string".to_string());
        }
        if key.len() > MAX_KEY_BYTES {
            return Err(format!(
                "artifact: key exceeds {} bytes (got {})",
                MAX_KEY_BYTES,
                key.len()
            ));
        }
        if mime_type.is_empty() || !mime_type.contains('/') {
            return Err(format!(
                "artifact: mime must look like \"type/subtype\" (e.g. \"image/png\"), got {:?}",
                mime_type
            ));
        }
        if mime_type.len() > MAX_MIME_BYTES {
            return Err(format!(
                "artifact: mime exceeds {} bytes (got {})",
                MAX_MIME_BYTES,
                mime_type.len()
            ));
        }
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(format!(
                "artifact: payload exceeds {} bytes (got {})",
                MAX_ARTIFACT_BYTES,
                bytes.len()
            ));
        }

        let header = StoredHeader {
            mime_type: mime_type.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            execution_id: execution_id.map(str::to_string),
        };
        let header_json = serde_json::to_vec(&header)
            .map_err(|e| format!("artifact: failed to encode header: {}", e))?;

        let mut value = Vec::with_capacity(4 + header_json.len() + bytes.len());
        value.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
        value.extend_from_slice(&header_json);
        value.extend_from_slice(bytes);

        self.tree
            .insert(key.as_bytes(), value)
            .map_err(|e| format!("artifact: failed to store '{}': {}", key, e))?;

        Ok(ArtifactMeta {
            key: key.to_string(),
            mime_type: header.mime_type,
            size_bytes: bytes.len() as u64,
            created_at: header.created_at,
            execution_id: header.execution_id,
        })
    }

    /// Fetch an artifact by key. `Ok(None)` when the key doesn't exist.
    pub fn get(&self, key: &str) -> Result<Option<Artifact>, String> {
        let Some(value) = self
            .tree
            .get(key.as_bytes())
            .map_err(|e| format!("artifact: failed to read '{}': {}", key, e))?
        else {
            return Ok(None);
        };
        let (meta, payload_start) = decode_header(key, &value)?;
        Ok(Some(Artifact {
            meta,
            bytes: value[payload_start..].to_vec(),
        }))
    }

    /// List metadata for all stored artifacts, sorted by key.
    pub fn list(&self) -> Result<Vec<ArtifactMeta>, String> {
        let mut out = Vec::new();
        for item in self.tree.iter() {
            let (k, value) =
                item.map_err(|e| format!("artifact: failed to iterate store: {}", e))?;
            let key = String::from_utf8_lossy(&k).to_string();
            let (meta, _) = decode_header(&key, &value)?;
            out.push(meta);
        }
        Ok(out)
    }
}

fn decode_header(key: &str, value: &[u8]) -> Result<(ArtifactMeta, usize), String> {
    if value.len() < 4 {
        return Err(format!("artifact: corrupt entry for '{}'", key));
    }
    let header_len = u32::from_be_bytes([value[0], value[1], value[2], value[3]]) as usize;
    let payload_start = 4 + header_len;
    if value.len() < payload_start {
        return Err(format!("artifact: corrupt entry for '{}'", key));
    }
    let header: StoredHeader = serde_json::from_slice(&value[4..payload_start])
        .map_err(|e| format!("artifact: corrupt header for '{}': {}", key, e))?;
    Ok((
        ArtifactMeta {
            key: key.to_string(),
            mime_type: header.mime_type,
            size_bytes: (value.len() - payload_start) as u64,
            created_at: header.created_at,
            execution_id: header.execution_id,
        },
        payload_start,
    ))
}

// ── Per-execution OpState entry ──────────────────────────────────────────

/// Stored in deno_core's `OpState`; connects the `artifact()` op to the store
/// and records what the current execution emitted (shared with the spawning
/// task, which files it on the execution record after the run).
#[derive(Clone)]
pub struct ArtifactState {
    pub store: ArtifactStore,
    pub execution_id: Option<String>,
    pub emitted: Arc<Mutex<Vec<ArtifactMeta>>>,
}

impl ArtifactState {
    pub fn new(store: ArtifactStore, execution_id: Option<String>) -> Self {
        Self {
            store,
            execution_id,
            emitted: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

// ── Op definition ────────────────────────────────────────────────────────

/// Sync op: validate and store one artifact. Called from JS via the
/// `artifact(key, mime, bytes)` global.
#[op2(fast)]
fn op_artifact_write(
    state: &mut OpState,
    #[string] key: &str,
    #[string] mime: &str,
    #[buffer(copy)] data: Vec<u8>,
) -> Result<(), JsErrorBox> {
    let artifact_state = state.borrow::<ArtifactState>();
    let meta = artifact_state
        .store
        .put(key, mime, &data, artifact_state.execution_id.as_deref())
        .map_err(JsErrorBox::generic)?;
    let mut emitted = artifact_state.emitted.lock().unwrap();
    emitted.retain(|m| m.key != meta.key);
    emitted.push(meta);
    Ok(())
}

// ── Extension registration ───────────────────────────────────────────────

deno_core::extension!(
    artifacts_ext,
    ops = [op_artifact_write],
);

/// Create the artifacts extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    artifacts_ext::init()
}

// ── Inject the artifact JS global ────────────────────────────────────────

/// Install the `globalThis.artifact` wrapper. Must run after the runtime is
/// created (with the artifacts extension) but before user code runs.
pub fn inject_artifact(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<artifact-setup>", ARTIFACT_JS_WRAPPER.to_string())
        .map(|_| ())
        .map_err(|e| format!("Failed to install artifact wrapper: {}", e))
}

/// Overload for JsRuntimeForSnapshot (stateful mode).
pub fn inject_artifact_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script("<artifact-setup>", ARTIFACT_JS_WRAPPER.to_string())
        .map(|_| ())
        .map_err(|e| format!("Failed to install artifact wrapper: {}", e))
}

/// `artifact(key, mime, bytes)` — store an artifact for the MCP client.
/// Accepts a Uint8Array/TypedArray/ArrayBuffer payload, or a string (which is
/// UTF-8 encoded). Same key overwrites.
const ARTIFACT_JS_WRAPPER: &str = r#"
(function() {
    globalThis.artifact = function artifact(key, mime, bytes) {
        if (typeof key !== 'string' || key.length === 0) {
            throw new TypeError('artifact: key must be a non-empty string');
        }
        if (typeof mime !== 'string' || mime.length === 0) {
            throw new TypeError('artifact: mime must be a non-empty string (e.g. "image/png")');
        }
        var u8;
        if (bytes instanceof Uint8Array) {
            u8 = bytes;
        } else if (bytes instanceof ArrayBuffer) {
            u8 = new Uint8Array(bytes);
        } else if (ArrayBuffer.isView(bytes)) {
            u8 = new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        } else if (typeof bytes === 'string') {
            u8 = new TextEncoder().encode(bytes);
        } else {
            throw new TypeError('artifact: bytes must be a Uint8Array, TypedArray, ArrayBuffer, or string');
        }
        Deno.core.ops.op_artifact_write(key, mime, u8);
    };
})();
"#;

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> ArtifactStore {
        let db = sled::Config::new().temporary(true).open().unwrap();
        ArtifactStore::new(db.open_tree("artifacts").unwrap())
    }

    #[test]
    fn put_get_roundtrip() {
        let store = temp_store();
        let meta = store
            .put("chart", "image/png", &[0x89, 0x50, 0x4e, 0x47], Some("exec-1"))
            .unwrap();
        assert_eq!(meta.key, "chart");
        assert_eq!(meta.mime_type, "image/png");
        assert_eq!(meta.size_bytes, 4);
        assert_eq!(meta.execution_id.as_deref(), Some("exec-1"));

        let artifact = store.get("chart").unwrap().unwrap();
        assert_eq!(artifact.bytes, vec![0x89, 0x50, 0x4e, 0x47]);
        assert_eq!(artifact.meta.mime_type, "image/png");
        assert!(matches!(
            artifact.content(),
            ArtifactContent::Image { ref mime_type, .. } if mime_type == "image/png"
        ));
    }

    #[test]
    fn same_key_overwrites() {
        let store = temp_store();
        store.put("k", "text/plain", b"one", None).unwrap();
        store.put("k", "text/plain", b"two", None).unwrap();
        let artifact = store.get("k").unwrap().unwrap();
        assert_eq!(artifact.bytes, b"two");
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn missing_key_is_none() {
        let store = temp_store();
        assert!(store.get("nope").unwrap().is_none());
    }

    #[test]
    fn validation_errors() {
        let store = temp_store();
        assert!(store.put("", "image/png", b"x", None).is_err());
        assert!(store.put("k", "png", b"x", None).is_err());
        assert!(store.put(&"k".repeat(MAX_KEY_BYTES + 1), "image/png", b"x", None).is_err());
        let big = vec![0u8; MAX_ARTIFACT_BYTES + 1];
        assert!(store.put("k", "image/png", &big, None).is_err());
    }

    #[test]
    fn text_and_binary_rendering() {
        let store = temp_store();
        store.put("t", "text/csv", b"a,b\n1,2\n", None).unwrap();
        store.put("b", "application/octet-stream", &[0xff, 0xfe, 0x00], None).unwrap();

        let t = store.get("t").unwrap().unwrap();
        assert!(matches!(t.content(), ArtifactContent::Text(ref s) if s == "a,b\n1,2\n"));
        assert_eq!(t.encoding(), "utf-8");

        let b = store.get("b").unwrap().unwrap();
        assert!(matches!(b.content(), ArtifactContent::Base64(_)));
        assert_eq!(b.encoding(), "base64");
    }

    #[test]
    fn list_returns_sorted_metadata() {
        let store = temp_store();
        store.put("z", "text/plain", b"z", None).unwrap();
        store.put("a", "image/png", b"aa", None).unwrap();
        let metas = store.list().unwrap();
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].key, "a");
        assert_eq!(metas[0].size_bytes, 2);
        assert_eq!(metas[1].key, "z");
    }
}
