pub mod types;
pub mod store;
pub mod gc;

use std::collections::BTreeMap;
use std::sync::Arc;

use openraft::BasicNode;
use openraft::Config;
use openraft::Raft;
use openraft::RaftNetworkFactory;
use openraft::SnapshotPolicy;
use openraft::error::InstallSnapshotError;
use openraft::error::RPCError;
use openraft::error::RaftError;
use openraft::network::RPCOption;
use openraft::network::RaftNetwork;
use openraft::storage::Adaptor;

use crate::raft::store::SledStore;
use crate::raft::types::TypeConfig;

pub type RaftNode = Raft<TypeConfig>;

/// A no-op network implementation for single-node mode.
#[derive(Clone, Default)]
pub struct NoopNetwork;

impl RaftNetworkFactory<TypeConfig> for NoopNetwork {
    type Network = NoopNetworkConnection;

    async fn new_client(
        &mut self,
        _target: u64,
        _node: &BasicNode,
    ) -> Self::Network {
        NoopNetworkConnection
    }
}

pub struct NoopNetworkConnection;

impl RaftNetwork<TypeConfig> for NoopNetworkConnection {
    async fn append_entries(
        &mut self,
        _rpc: openraft::raft::AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        openraft::raft::AppendEntriesResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64>>,
    > {
        unreachable!("single-node cluster does not use network")
    }

    async fn install_snapshot(
        &mut self,
        _rpc: openraft::raft::InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        openraft::raft::InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        unreachable!("single-node cluster does not use network")
    }

    async fn vote(
        &mut self,
        _rpc: openraft::raft::VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<
        openraft::raft::VoteResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64>>,
    > {
        unreachable!("single-node cluster does not use network")
    }
}

/// Start a single-node Raft cluster backed by sled.
///
/// Returns the Raft handle (for proposals) and the store (for reads).
pub async fn start_raft_node(
    db_path: &str,
) -> anyhow::Result<(RaftNode, SledStore)> {
    let config = Arc::new(
        Config {
            heartbeat_interval: 500,
            election_timeout_min: 1500,
            election_timeout_max: 3000,
            snapshot_policy: SnapshotPolicy::LogsSinceLast(1000),
            ..Default::default()
        }
    );

    let db = sled::open(db_path)?;
    let store = SledStore::new(db)?;
    let store_clone = store.clone();

    let (log_store, state_machine) = Adaptor::new(store);
    let network = NoopNetwork;

    let raft = Raft::new(0, config, network, log_store, state_machine).await?;

    // Initialize as sole voter. Safe to call multiple times — openraft
    // ignores if already initialized.
    let mut members = BTreeMap::new();
    members.insert(0, BasicNode::default());
    let _ = raft.initialize(members).await;

    Ok((raft, store_clone))
}

/// Start a single-node Raft cluster backed by sled in temporary mode (for testing).
pub async fn start_raft_node_temp() -> anyhow::Result<(RaftNode, SledStore)> {
    let config = Arc::new(
        Config {
            heartbeat_interval: 500,
            election_timeout_min: 1500,
            election_timeout_max: 3000,
            snapshot_policy: SnapshotPolicy::LogsSinceLast(1000),
            ..Default::default()
        }
    );

    let db = sled::Config::new().temporary(true).open()?;
    let store = SledStore::new(db)?;
    let store_clone = store.clone();

    let (log_store, state_machine) = Adaptor::new(store);
    let network = NoopNetwork;

    let raft = Raft::new(0, config, network, log_store, state_machine).await?;

    let mut members = BTreeMap::new();
    members.insert(0, BasicNode::default());
    let _ = raft.initialize(members).await;

    Ok((raft, store_clone))
}
