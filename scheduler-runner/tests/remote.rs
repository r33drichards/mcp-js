use chrono::{Duration as Delta, Utc};
use http_body_util::{BodyExt, Full};
use hyper::{Response, body::Bytes, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use mcp_js_scheduler_core::Outcome;
use mcp_js_scheduler_runner::{RemoteConfig, RemoteExecutor};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn invocation() -> mcp_js_scheduler_runner::scheduled_protocol::Dispatch {
    use mcp_js_scheduler_runner::scheduled_protocol::*;
    let payload = Payload {
        code: "1+1".into(),
        session: "operator-test".into(),
        timeout_seconds: 1,
        heap_mb: 32,
        expires_at: Utc::now() + Delta::hours(1),
    };
    Dispatch {
        version: 1,
        namespace: "test".into(),
        worker_id: "worker".into(),
        owner: "n1".into(),
        epoch: 1,
        authority: "test".into(),
        schedule_id: 1,
        occurrence_id: 1,
        attempt: 1,
        payload_digest: digest(&payload),
        payload,
    }
}
fn config(url: String) -> RemoteConfig {
    RemoteConfig {
        url,
        namespace: "test".into(),
        worker_id: "worker".into(),
        bearer_env: "PATH".into(),
        expires_at: Utc::now() + Delta::hours(1),
        session: "operator-test".into(),
        timeout_seconds: 1,
        heap_mb: 32,
    }
}
#[tokio::test]
async fn accepted_is_not_completion_and_submission_is_never_repeated() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let posts = Arc::new(AtomicUsize::new(0));
    let gets = Arc::new(AtomicUsize::new(0));
    let (p, g) = (posts.clone(), gets.clone());
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let (p, g) = (p.clone(), g.clone());
            tokio::spawn(async move {
                http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(socket),
                        service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                            let (p, g) = (p.clone(), g.clone());
                            async move {
                                assert!(req.headers().contains_key("authorization"));
                                let (status, body) = if req.method() == hyper::Method::POST {
                                    assert_eq!(req.uri().path(), "/api/scheduled/v1/exec");
                                    p.fetch_add(1, Ordering::SeqCst);
                                    let body = req.into_body().collect().await.unwrap().to_bytes();
                                    let value: serde_json::Value =
                                        serde_json::from_slice(&body).unwrap();
                                    assert_eq!(value["payload"]["code"], "1+1");
                                    (202, r#"{"execution_id":"work-1"}"#)
                                } else if g.fetch_add(1, Ordering::SeqCst) == 0 {
                                    (200, r#"{"execution_id":"work-1","status":"running"}"#)
                                } else {
                                    (200, r#"{"execution_id":"work-1","status":"completed"}"#)
                                };
                                Ok::<_, Infallible>(
                                    Response::builder()
                                        .status(status)
                                        .body(Full::new(Bytes::from_static(body.as_bytes())))
                                        .unwrap(),
                                )
                            }
                        }),
                    )
                    .await
                    .unwrap();
            });
        }
    });
    let executor = RemoteExecutor::new(config(url)).unwrap();
    assert!(matches!(
        executor.execute_authorized(invocation()).await,
        Outcome::Succeeded
    ));
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    assert_eq!(gets.load(Ordering::SeqCst), 2);
    server.abort();
}
#[tokio::test]
async fn lost_submission_response_is_unknown_without_retry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 8192];
            let _ = socket.read(&mut buffer).await;
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    assert!(matches!(
        RemoteExecutor::new(config(url))
            .unwrap()
            .execute_authorized(invocation())
            .await,
        Outcome::Unknown(_)
    ));
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    server.abort();
}
#[tokio::test]
async fn expired_grant_never_contacts_worker() {
    let mut c = config("http://127.0.0.1:1".into());
    c.expires_at = Utc::now() - Delta::seconds(1);
    assert!(matches!(
        RemoteExecutor::new(c)
            .unwrap()
            .execute_authorized(invocation())
            .await,
        Outcome::Failed(_)
    ));
}
#[test]
fn credential_urls_and_remote_plaintext_are_rejected() {
    assert!(RemoteExecutor::new(config("http://example.com".into())).is_err());
    assert!(RemoteExecutor::new(config("https://secret@example.com".into())).is_err());
}

#[tokio::test]
async fn terminal_status_and_malformed_ack_classification() {
    for (status, expected) in [
        ("failed", "failed"),
        ("timed_out", "unknown"),
        ("cancelled", "unknown"),
        ("malformed_ack", "unknown"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let posts = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let count = count.clone();
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(socket),
                            service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                let count = count.clone();
                                async move {
                                    let (code, body) = if req.method() == hyper::Method::POST {
                                        count.fetch_add(1, Ordering::SeqCst);
                                        let _ = req.into_body().collect().await;
                                        (
                                            202,
                                            if status == "malformed_ack" {
                                                "{}".into()
                                            } else {
                                                r#"{"execution_id":"work-1"}"#.into()
                                            },
                                        )
                                    } else {
                                        (
                                            200,
                                            format!(
                                                r#"{{"execution_id":"work-1","status":"{status}"}}"#
                                            ),
                                        )
                                    };
                                    Ok::<_, Infallible>(
                                        Response::builder()
                                            .status(code)
                                            .body(Full::new(Bytes::from(body)))
                                            .unwrap(),
                                    )
                                }
                            }),
                        )
                        .await;
                });
            }
        });
        let result = RemoteExecutor::new(config(url))
            .unwrap()
            .execute_authorized(invocation())
            .await;
        assert!(
            match expected {
                "failed" => matches!(result, Outcome::Failed(_)),
                _ => matches!(result, Outcome::Unknown(_)),
            },
            "{status}: {result:?}"
        );
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
