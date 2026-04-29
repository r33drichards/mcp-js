//! Temporal integration: durable background and scheduled JS task execution.
//!
//! The MCP server does not embed a Temporal client itself — the official Rust
//! SDK transitively requires `serde >= 1.0.225`, which conflicts with the
//! `swc_common 8.1.1` pin in this crate. Instead we run a tiny Python sidecar
//! (see `temporal_worker/`) that:
//!
//!   * Hosts the Temporal worker that executes `RunJsWorkflow` and
//!     `ScheduledRunJsWorkflow`.
//!   * Calls back into this server's `/api/exec` HTTP endpoint to actually run
//!     the JavaScript.
//!   * Exposes a small REST control plane on `--temporal-tasks-url` that this
//!     module wraps. That control plane lets us submit ad-hoc background runs
//!     and create / pause / delete per-session schedules.
//!
//! When `--temporal-tasks-url` is not configured, all of these tools are
//! skipped and `run_js` continues to call the engine directly.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TemporalTasksClient {
    base: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunJsInput {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_memory_max_mb: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_headers: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
struct CreateScheduleRequest<'a> {
    session: &'a str,
    schedule_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cron: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    every_secs: Option<u64>,
    input: &'a RunJsInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    paused: bool,
}

impl TemporalTasksClient {
    pub fn new(base: impl Into<String>) -> Result<Self> {
        let base = base.into().trim_end_matches('/').to_string();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| anyhow!("temporal-tasks: build http client: {e}"))?;
        Ok(Self { base, http })
    }

    /// Health-check the sidecar on startup so misconfiguration fails fast.
    pub async fn health(&self) -> Result<()> {
        let url = format!("{}/health", self.base);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: GET /health: {e}"))?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "temporal-tasks: /health returned {}",
                resp.status()
            ));
        }
        Ok(())
    }

    /// Submit an ad-hoc background `run_js` workflow. Returns the workflow_id
    /// (which the caller treats as the MCP execution_id).
    pub async fn submit_run_js(&self, input: &RunJsInput) -> Result<String> {
        let url = format!("{}/workflows/run_js", self.base);
        let resp = self
            .http
            .post(&url)
            .json(input)
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: POST /workflows/run_js: {e}"))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("temporal-tasks: decode body: {e}"))?;
        if !status.is_success() {
            return Err(anyhow!("temporal-tasks: workflow start failed: {body}"));
        }
        body.get("workflow_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("temporal-tasks: missing workflow_id in response: {body}"))
    }

    pub async fn create_schedule(
        &self,
        session: &str,
        schedule_id: &str,
        cron: Option<&str>,
        every_secs: Option<u64>,
        input: &RunJsInput,
        note: Option<&str>,
        paused: bool,
    ) -> Result<serde_json::Value> {
        if cron.is_none() && every_secs.is_none() {
            return Err(anyhow!("either `cron` or `every_secs` must be provided"));
        }
        let url = format!("{}/schedules", self.base);
        let body = CreateScheduleRequest {
            session,
            schedule_id,
            cron,
            every_secs,
            input,
            note,
            paused,
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: POST /schedules: {e}"))?;
        let status = resp.status();
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("temporal-tasks: decode body: {e}"))?;
        if !status.is_success() {
            return Err(anyhow!("temporal-tasks: create schedule failed: {json}"));
        }
        Ok(json)
    }

    pub async fn list_schedules(&self, session: &str) -> Result<serde_json::Value> {
        let url = format!("{}/schedules", self.base);
        let resp = self
            .http
            .get(&url)
            .query(&[("session", session)])
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: GET /schedules: {e}"))?;
        let status = resp.status();
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow!("temporal-tasks: decode body: {e}"))?;
        if !status.is_success() {
            return Err(anyhow!("temporal-tasks: list schedules failed: {json}"));
        }
        Ok(json)
    }

    pub async fn delete_schedule(&self, session: &str, schedule_id: &str) -> Result<()> {
        let url = format!(
            "{}/schedules/{}",
            self.base,
            urlencode(&format!("{session}/{schedule_id}"))
        );
        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: DELETE /schedules: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("temporal-tasks: delete failed ({status}): {body}"));
        }
        Ok(())
    }

    async fn pause_or_unpause(
        &self,
        session: &str,
        schedule_id: &str,
        action: &str,
        note: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/schedules/{}/{}",
            self.base,
            urlencode(&format!("{session}/{schedule_id}")),
            action
        );
        let mut body = serde_json::Map::new();
        if let Some(n) = note {
            body.insert("note".to_string(), serde_json::Value::String(n.to_string()));
        }
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: POST {url}: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("temporal-tasks: {action} failed ({status}): {body}"));
        }
        Ok(())
    }

    pub async fn pause_schedule(
        &self,
        session: &str,
        schedule_id: &str,
        note: Option<&str>,
    ) -> Result<()> {
        self.pause_or_unpause(session, schedule_id, "pause", note)
            .await
    }

    pub async fn unpause_schedule(
        &self,
        session: &str,
        schedule_id: &str,
        note: Option<&str>,
    ) -> Result<()> {
        self.pause_or_unpause(session, schedule_id, "unpause", note)
            .await
    }

    pub async fn trigger_schedule(&self, session: &str, schedule_id: &str) -> Result<()> {
        let url = format!(
            "{}/schedules/{}/trigger",
            self.base,
            urlencode(&format!("{session}/{schedule_id}"))
        );
        let resp = self
            .http
            .post(&url)
            .send()
            .await
            .map_err(|e| anyhow!("temporal-tasks: POST {url}: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("temporal-tasks: trigger failed ({status}): {body}"));
        }
        Ok(())
    }
}

/// Minimal percent-encoder for path segments — we only allow alphanumerics,
/// `-`, `_`, `.`, `~`, and percent-encode the rest. This avoids a `urlencoding`
/// dep just for two callers.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b;
        let is_unreserved = c.is_ascii_alphanumeric()
            || c == b'-'
            || c == b'_'
            || c == b'.'
            || c == b'~';
        if is_unreserved {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{:02X}", c));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_path_safely() {
        assert_eq!(urlencode("alice/nightly"), "alice%2Fnightly");
        assert_eq!(urlencode("plain"), "plain");
    }
}
