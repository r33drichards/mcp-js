//! Per-execution request/response channel for tools implemented by a foreign host.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{RuntimeError, ToolDefinition};

const MAX_CALLS: u32 = 32;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(uniffi::Record)]
pub struct HostToolCall {
    pub id: u32,
    pub name: String,
    pub arguments_json: String,
}

type Response = oneshot::Sender<(String, bool)>;

#[derive(uniffi::Object)]
pub struct HostToolRun {
    server: String,
    names: HashSet<String>,
    catalog: String,
    calls: AtomicU32,
    started: AtomicBool,
    requests: mpsc::Sender<HostToolCall>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<HostToolCall>>,
    pending: Mutex<HashMap<u32, Response>>,
    closed: CancellationToken,
    stopped: CancellationToken,
}

#[uniffi::export]
impl HostToolRun {
    #[uniffi::constructor]
    pub fn create(server: String, tools: Vec<ToolDefinition>) -> Result<Arc<Self>, RuntimeError> {
        Self::new(server, tools).map_err(|message| RuntimeError::InvalidConfig { message })
    }

    pub async fn next_call(&self) -> Option<HostToolCall> {
        let mut receiver = self.receiver.lock().await;
        tokio::select! {
            biased;
            _ = self.stopped.cancelled() => None,
            request = receiver.recv() => request,
        }
    }

    pub fn respond(&self, id: u32, payload: String, is_error: bool) -> bool {
        let Some(sender) = self.pending.lock().unwrap().remove(&id) else {
            return false;
        };
        let response = if payload.len() > MAX_PAYLOAD_BYTES {
            ("Host tool result exceeds the size limit".into(), true)
        } else {
            (payload, is_error)
        };
        sender.send(response).is_ok()
    }

    pub fn close(&self) {
        self.closed.cancel();
        self.stop_calls();
    }
}

impl HostToolRun {
    pub(super) fn new(server: String, tools: Vec<ToolDefinition>) -> Result<Arc<Self>, String> {
        if server.is_empty() || server.contains("__") {
            return Err("Host tool server must be nonempty and cannot contain '__'".into());
        }
        let mut names = HashSet::new();
        let mut catalog = Vec::new();
        for tool in tools {
            if tool.name.is_empty() || !names.insert(tool.name.clone()) {
                return Err("Host tool names must be nonempty and unique".into());
            }
            let schema = serde_json::from_str::<Value>(&tool.input_schema_json)
                .map_err(|error| format!("Invalid schema for {}: {error}", tool.name))?;
            if !schema.is_object() {
                return Err(format!("Expected an object schema for {}", tool.name));
            }
            catalog.push(json!({
                "server": server, "name": tool.name, "description": tool.description,
                "inputSchema": schema,
            }));
        }
        let (requests, receiver) = mpsc::channel(MAX_CALLS as usize);
        Ok(Arc::new(Self {
            server,
            names,
            catalog: Value::Array(catalog).to_string(),
            calls: AtomicU32::new(0),
            started: AtomicBool::new(false),
            requests,
            receiver: receiver.into(),
            pending: Mutex::new(HashMap::new()),
            closed: CancellationToken::new(),
            stopped: CancellationToken::new(),
        }))
    }

    pub(super) fn start(&self) -> Result<(), RuntimeError> {
        if self.closed.is_cancelled() || self.started.swap(true, Ordering::Relaxed) {
            return Err(RuntimeError::Operation {
                message: "Host tool run can only be used once".into(),
            });
        }
        Ok(())
    }

    pub(super) fn stop_calls(&self) {
        self.stopped.cancel();
        self.pending.lock().unwrap().clear();
    }

    pub(super) async fn cancelled(&self) {
        self.closed.cancelled().await;
    }

    pub(super) fn list(&self, server: &str) -> String {
        if server.is_empty() || server == self.server {
            self.catalog.clone()
        } else {
            "[]".into()
        }
    }

    pub(super) fn servers(&self) -> String {
        json!([self.server]).to_string()
    }

    pub(super) async fn call(
        &self,
        server: String,
        name: String,
        arguments_json: String,
    ) -> Result<String, String> {
        if server != self.server || !self.names.contains(&name) {
            return Err("Unknown host tool".into());
        }
        if arguments_json.len() > MAX_PAYLOAD_BYTES {
            return Err("Host tool arguments exceed the size limit".into());
        }
        let id = self
            .calls
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_CALLS).then_some(count + 1)
            })
            .map_err(|_| "Host tool call limit exceeded")?;
        let arguments_json = if arguments_json.is_empty() {
            "{}".into()
        } else {
            arguments_json
        };
        if !serde_json::from_str::<Value>(&arguments_json).is_ok_and(|value| value.is_object()) {
            return Err("Host tool arguments must be a JSON object".into());
        }
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        let _pending = PendingCall(self, id);
        self.requests
            .try_send(HostToolCall {
                id,
                name,
                arguments_json,
            })
            .map_err(|_| "Host tool request queue unavailable")?;
        let (payload, is_error) = tokio::select! {
            _ = self.stopped.cancelled() => return Err("Host tool run closed".into()),
            result = receiver => result.map_err(|_| "Host tool response dropped")?,
        };
        Ok(
            json!({"content": [{"type": "text", "text": payload}], "isError": is_error})
                .to_string(),
        )
    }
}

struct PendingCall<'a>(&'a HostToolRun, u32);

impl Drop for PendingCall<'_> {
    fn drop(&mut self) {
        self.0.pending.lock().unwrap().remove(&self.1);
    }
}
