use crate::scheduled_protocol::Dispatch;
use chrono::{DateTime, Utc};
use mcp_js_scheduler_core::{Outcome, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Operator-owned execution authority; only the credential reference is durable.
/// The worker still validates its usual bearer credential and execution policies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    pub namespace: String,
    pub worker_id: String,
    pub bearer_env: String,
    pub expires_at: DateTime<Utc>,
    pub session: String,
    pub timeout_seconds: u64,
    pub heap_mb: usize,
}
pub struct RemoteExecutor {
    config: RemoteConfig,
    client: reqwest::Client,
}
impl RemoteExecutor {
    pub fn new(config: RemoteConfig) -> Result<Self> {
        let url = reqwest::Url::parse(&config.url).map_err(|e| e.to_string())?;
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
        {
            return Err("worker must use HTTPS or loopback HTTP".into());
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err("worker URL must be an origin without credentials".into());
        }
        if config.namespace.is_empty()
            || config.worker_id.is_empty()
            || config.bearer_env.is_empty()
            || config.session.is_empty()
            || config.timeout_seconds == 0
            || config.timeout_seconds > 300
            || config.heap_mb < 16
            || config.heap_mb > 4096
        {
            return Err("invalid execution authority or limits".into());
        }
        let client = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { config, client })
    }
    pub async fn execute_authorized(&self, dispatch: Dispatch) -> Outcome {
        if dispatch.validate().is_err()
            || dispatch.namespace != self.config.namespace
            || dispatch.worker_id != self.config.worker_id
        {
            return Outcome::Failed("invalid scheduled dispatch binding".into());
        }
        if Utc::now() >= self.config.expires_at {
            return Outcome::Failed("operator grant expired before submission".into());
        }
        let token = match std::env::var(&self.config.bearer_env) {
            Ok(token) if !token.is_empty() => token,
            _ => {
                return Outcome::Failed(
                    "credential reference unavailable before submission".into(),
                );
            }
        };
        let origin = self.config.url.trim_end_matches('/');
        // Exactly one submission attempt. Transport errors, non-202 and malformed
        // responses are ambiguous, never a signal to retry an external execution.
        let response = self
            .client
            .post(format!("{origin}/api/scheduled/v1/exec"))
            .bearer_auth(&token)
            .json(&dispatch)
            .send()
            .await;
        let Ok(response) = response else {
            return Outcome::Unknown("submission response lost".into());
        };
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Outcome::Unknown("worker did not acknowledge submission".into());
        }
        #[derive(Deserialize)]
        struct Accepted {
            execution_id: String,
        }
        let Ok(accepted) = response.json::<Accepted>().await else {
            return Outcome::Unknown("invalid submission acknowledgement".into());
        };
        if accepted.execution_id.is_empty()
            || !accepted
                .execution_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Outcome::Unknown("invalid execution identity".into());
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.timeout_seconds + 30);
        loop {
            if tokio::time::Instant::now() >= deadline || Utc::now() >= self.config.expires_at {
                return Outcome::Unknown(
                    "observation deadline or grant expiry; execution may continue".into(),
                );
            }
            let response = self
                .client
                .get(format!(
                    "{origin}/api/scheduled/v1/executions/{}",
                    accepted.execution_id
                ))
                .bearer_auth(&token)
                .send()
                .await;
            let Ok(response) = response else {
                return Outcome::Unknown("execution status unavailable".into());
            };
            if !response.status().is_success() {
                return Outcome::Unknown("execution status lost or access revoked".into());
            }
            #[derive(Deserialize)]
            struct Status {
                execution_id: String,
                status: String,
            }
            let Ok(status) = response.json::<Status>().await else {
                return Outcome::Unknown("invalid execution status".into());
            };
            if status.execution_id != accepted.execution_id {
                return Outcome::Unknown("execution status identity mismatch".into());
            }
            match status.status.as_str() {
                "completed" => return Outcome::Succeeded,
                "failed" => return Outcome::Failed("worker confirmed execution failure".into()),
                "running" => tokio::time::sleep(Duration::from_millis(250)).await,
                // Cancellation/timeouts do not establish that arbitrary external effects stopped.
                _ => return Outcome::Unknown("worker reported an uncertain termination".into()),
            }
        }
    }
}
