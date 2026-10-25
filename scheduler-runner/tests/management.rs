//! Real runner process, HTTP listener and MCP transport. No live credentials.
use chrono::{Duration as Delta, Utc};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::process::{Child, Command};

const OPERATOR: &str = "disposable-test-operator";
const RECOVERY: &str = "disposable-test-recovery";
struct Runner {
    _dir: tempfile::TempDir,
    child: Child,
    url: String,
    client: reqwest::Client,
}
impl Runner {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cluster = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let management = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = management.local_addr().unwrap();
        let cluster_addr = cluster.local_addr().unwrap();
        let config = json!({"node_id":"test", "advertise_addr":cluster_addr.to_string(), "cluster_port":cluster_addr.port(), "peers":[], "data_dir":dir.path().join("data"), "admission_addr":"127.0.0.1:0", "admission_bearer_env":"TEST_ADMISSION", "management":{"addr":addr.to_string(),"bearer_env":"TEST_OPERATOR","recovery_bearer_env":"TEST_RECOVERY"}, "remote":{"url":"http://127.0.0.1:1","namespace":"test","worker_id":"test","bearer_env":"TEST_WORKER","expires_at":Utc::now()+Delta::hours(1),"session":"test","timeout_seconds":1,"heap_mb":32}});
        let path = dir.path().join("config.json");
        std::fs::write(&path, config.to_string()).unwrap();
        drop(cluster);
        drop(management);
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-js-scheduler-runner"))
            .arg(path)
            .env("TEST_OPERATOR", OPERATOR)
            .env("TEST_RECOVERY", RECOVERY)
            .env("TEST_WORKER", "disposable-test-worker")
            .env("TEST_ADMISSION", "disposable-test-admission")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let runner = Self {
            _dir: dir,
            child,
            url: format!("http://{addr}"),
            client: reqwest::Client::new(),
        };
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(response) = runner
                    .client
                    .get(format!("{}/scheduler/v1/status", runner.url))
                    .bearer_auth(OPERATOR)
                    .send()
                    .await
                    && response.status().is_success()
                    && response.json::<Value>().await.unwrap()["owner_acquired"] == true
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("acquired owner");
        runner
    }
    async fn http(&self, token: &str, command: Value) -> (u16, Value) {
        let response = self
            .client
            .post(format!("{}/scheduler/v1/commands", self.url))
            .bearer_auth(token)
            .json(&command)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
    async fn rpc(&self, token: &str, method: &str, params: Value) -> Value {
        let response = self
            .client
            .post(format!("{}/mcp", self.url))
            .bearer_auth(token)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert!(status.is_success(), "MCP {status}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|_| panic!("MCP response: {body}"))
    }
    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let response = self
            .rpc(
                token,
                "tools/call",
                json!({"name":format!("scheduler_{name}"),"arguments":arguments}),
            )
            .await;
        assert!(response.get("error").is_none(), "{response}");
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    async fn list(&self) -> Value {
        self.client
            .get(format!("{}/scheduler/v1/schedules", self.url))
            .bearer_auth(OPERATOR)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}
fn definition() -> Value {
    json!({"name":"test", "code":"1+1", "cron":"* * * * *", "timezone":"UTC", "policy":{"missed":"RunOnce","overlap":"Queue","retry":{"max_retries":0,"delay_seconds":0}}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_http_mcp_auth_validation_and_management() {
    let mut runner = Runner::start().await;
    for path in [
        "/scheduler/v1/status",
        "/scheduler/v1/schedules",
        "/scheduler/v1/commands",
        "/mcp",
    ] {
        for token in [
            "wrong",
            "disposable-test-admission",
            "disposable-test-worker",
        ] {
            let response = runner
                .client
                .post(format!("{}{path}", runner.url))
                .bearer_auth(token)
                .body("invalid json")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 401, "{path}");
            assert_eq!(
                response.json::<Value>().await.unwrap(),
                json!({"error":"unauthorized"})
            );
        }
    }
    assert_eq!(
        runner
            .http(OPERATOR, json!({"action":"create","definition":{}}))
            .await
            .0,
        400
    );
    assert_eq!(
        runner
            .http(OPERATOR, json!({"action":"disable","id":-1}))
            .await
            .0,
        400
    );
    assert_eq!(
        runner
            .http(OPERATOR, json!({"action":"status","extra":"secret"}))
            .await
            .0,
        400
    );
    assert_eq!(runner.http(OPERATOR,json!({"action":"acknowledge_stopped","occurrence_id":1,"dispatcher_and_work_quiesced":true})).await.0,403);
    assert_eq!(runner.http(RECOVERY,json!({"action":"acknowledge_stopped","occurrence_id":1,"dispatcher_and_work_quiesced":false})).await.0,400);
    assert_eq!(
        runner
            .http(
                RECOVERY,
                json!({"action":"acknowledge_stopped","occurrence_id":1})
            )
            .await
            .0,
        400
    );
    let origin = runner
        .client
        .get(format!("{}/scheduler/v1/status", runner.url))
        .bearer_auth(OPERATOR)
        .header("origin", "https://example.org")
        .send()
        .await
        .unwrap();
    assert_eq!(origin.status(), 403);
    let large = runner
        .client
        .post(format!("{}/scheduler/v1/commands", runner.url))
        .bearer_auth(OPERATOR)
        .body("x".repeat(73 * 1024))
        .send()
        .await
        .unwrap();
    assert_eq!(large.status(), 413);
    let tools = runner.rpc(OPERATOR, "tools/list", json!({})).await;
    assert_eq!(
        tools["result"]["tools"].as_array().unwrap().len(),
        6,
        "{tools}"
    );
    let tools = runner.rpc(RECOVERY, "tools/list", json!({})).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 7);
    let denial = runner
        .tool(
            OPERATOR,
            "acknowledge_stopped",
            json!({"occurrence_id":1,"dispatcher_and_work_quiesced":true}),
        )
        .await;
    assert_eq!(denial["status"], 403);
    let invalid = runner
        .rpc(
            OPERATOR,
            "tools/call",
            json!({"name":"scheduler_disable","arguments":{"id":1,"extra":"no"}}),
        )
        .await;
    assert_eq!(invalid["error"]["code"], -32602);
    assert_eq!(
        runner.tool(OPERATOR, "status", json!({})).await["owner_acquired"],
        true
    );
    let (status, created) = runner
        .http(
            OPERATOR,
            json!({"action":"create","definition":definition()}),
        )
        .await;
    assert_eq!(status, 200);
    let id = created["id"].as_u64().unwrap();
    assert_eq!(
        runner.tool(OPERATOR, "list", json!({})).await["schedules"][0]["id"],
        id
    );
    assert_eq!(
        runner.tool(OPERATOR, "disable", json!({"id":id})).await["ok"],
        true
    );
    assert_eq!(runner.list().await["schedules"][0]["enabled"], false);
    assert_eq!(
        runner
            .http(OPERATOR, json!({"action":"delete","id":id}))
            .await
            .0,
        200
    );
    let created = runner
        .tool(OPERATOR, "create", json!({"definition":definition()}))
        .await;
    assert!(created["id"].as_u64().unwrap() > id);
    assert_eq!(
        runner
            .tool(OPERATOR, "delete", json!({"id":created["id"]}))
            .await["ok"],
        true
    );
    let (status, error) = runner
        .http(OPERATOR, json!({"action":"disable","id":999}))
        .await;
    assert_eq!(status, 503);
    assert_eq!(
        error,
        json!({"error":"operation_failed","outcome":"uncertain","retry_safe":false})
    );
    // Errors do not echo source, paths, credential values or backend error details.
    let invalid_definition=runner.tool(OPERATOR,"create",json!({"definition":{"code":"secret-source","cron":"bad","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}})).await;
    assert_eq!(invalid_definition["error"], "operation_failed");
    runner.child.kill().await.unwrap();
    runner.child.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_after_disable_and_explicit_recovery_keep_existing_semantics() {
    let mut runner = Runner::start().await;
    let first = runner
        .tool(OPERATOR, "create", json!({"definition":definition()}))
        .await["id"]
        .as_u64()
        .unwrap();
    let second = runner
        .http(
            OPERATOR,
            json!({"action":"create","definition":definition()}),
        )
        .await
        .1["id"]
        .as_u64()
        .unwrap();
    let occurrences = tokio::time::timeout(Duration::from_secs(75), async {
        loop {
            let view = runner.list().await;
            let occurrences = view["occurrences"].as_array().unwrap();
            if occurrences.len() >= 2
                && occurrences
                    .iter()
                    .all(|o| o["status"].get("Unknown").is_some())
            {
                break occurrences.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("real cron boundary and unreachable worker produce Unknown");
    let first_occurrence = occurrences
        .iter()
        .find(|o| o["schedule_id"] == first)
        .unwrap()["id"]
        .clone();
    let second_occurrence = occurrences
        .iter()
        .find(|o| o["schedule_id"] == second)
        .unwrap()["id"]
        .clone();
    assert_eq!(
        runner
            .http(OPERATOR, json!({"action":"disable","id":first}))
            .await
            .0,
        200
    );
    assert_eq!(
        runner
            .tool(
                OPERATOR,
                "cancel",
                json!({"occurrence_id":first_occurrence})
            )
            .await["cancellation_requested"],
        true
    );
    assert_eq!(
        runner
            .http(
                OPERATOR,
                json!({"action":"cancel","occurrence_id":first_occurrence})
            )
            .await
            .0,
        200
    );
    let view = runner.list().await;
    let first_record = view["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == first_occurrence)
        .unwrap();
    assert!(first_record["status"].get("Cancelling").is_some(), "{view}");
    // An explicit false attestation does not release Unknown capacity.
    assert_eq!(
        runner
            .tool(
                RECOVERY,
                "acknowledge_stopped",
                json!({"occurrence_id":second_occurrence,"dispatcher_and_work_quiesced":false})
            )
            .await["status"],
        400
    );
    assert_eq!(
        runner
            .tool(
                RECOVERY,
                "acknowledge_stopped",
                json!({"occurrence_id":second_occurrence,"dispatcher_and_work_quiesced":true})
            )
            .await["ok"],
        true
    );
    let view = runner.list().await;
    assert!(
        view["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == second_occurrence)
            .unwrap()["status"]
            .get("Abandoned")
            .is_some()
    );
    assert_eq!(
        view["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == second)
            .unwrap()["enabled"],
        true
    );
    // This stack deliberately does not import #284's newer Cancelling recovery semantics.
    assert_eq!(runner.http(RECOVERY,json!({"action":"acknowledge_stopped","occurrence_id":first_occurrence,"dispatcher_and_work_quiesced":true})).await.0,503);
    assert!(
        runner.list().await["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == first_occurrence)
            .unwrap()["status"]
            .get("Cancelling")
            .is_some()
    );
    runner.child.kill().await.unwrap();
    runner.child.wait().await.unwrap();
}
