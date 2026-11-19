//! Cluster HTTP requests over simulated TCP, with an injected network partition.
use scheduler_raft_audit::cluster::{ClusterConfig, ClusterNode, serve_cluster_connection};
use serde_json::Value;
use std::{error::Error, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use turmoil::net::{TcpListener, TcpStream};

const PORT: u16 = 4000;

async fn status() -> Result<Value, Box<dyn Error>> {
    let mut stream = TcpStream::connect(("node", PORT)).await?;
    stream
        .write_all(b"GET /raft/status HTTP/1.1\r\nHost: node\r\nConnection: close\r\n\r\n")
        .await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let response = String::from_utf8(response)?;
    let (head, body) = response.split_once("\r\n\r\n").ok_or("missing HTTP body")?;
    assert!(head.starts_with("HTTP/1.1 200"), "{response}");
    Ok(serde_json::from_str(body)?)
}

#[test]
fn cluster_status_recovers_after_network_partition() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(5))
        .build();
    sim.host("node", || async {
        let node = ClusterNode::new(
            ClusterConfig {
                node_id: "node".into(),
                ..ClusterConfig::default()
            },
            sled::Config::new().temporary(true).open()?,
        );
        let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
        loop {
            let (stream, _) = listener.accept().await?;
            let node = node.clone();
            tokio::spawn(async move {
                serve_cluster_connection(node, stream).await.unwrap();
            });
        }
    });
    sim.client("client", async {
        // A healthy request proves the listener was up before the fault.
        assert_eq!(status().await?["node_id"], "node");
        turmoil::partition("client", "node");
        let blocked = TcpStream::connect(("node", PORT)).await;
        assert_eq!(
            blocked.unwrap_err().kind(),
            std::io::ErrorKind::ConnectionRefused
        );
        turmoil::repair("client", "node");
        assert_eq!(status().await?["node_id"], "node");
        Ok(())
    });
    sim.run().unwrap();
}
