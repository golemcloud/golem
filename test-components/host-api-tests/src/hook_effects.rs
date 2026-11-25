//! A snapshotting agent whose load hook makes one effect, for the never-live tests. The
//! constructor names the effect and the port of the test peer, and the snapshot keeps both, so a
//! restored agent's load hook makes the same effect. The effect runs only in the load hook of an
//! agent other than the one that saved the snapshot, such as a fork, so that a source can restore
//! its own snapshot, for example in a manual update, without the effect. The snapshot keeps the ID
//! of the agent that saved it, and the load hook compares it with the agent's own ID.

use golem_rust::permissions::{self, Bounds};
use golem_rust::wasip3::sockets::types::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
use golem_rust::wasip3::sockets::types::{TcpSocket, UdpSocket};
use golem_rust::{AgentId, PromiseId, Uuid, agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct StoredPromise {
    agent_id: Uuid,
    fingerprint: Uuid,
    oplog_idx: u64,
}

impl From<&PromiseId> for StoredPromise {
    fn from(promise: &PromiseId) -> Self {
        Self {
            agent_id: promise.agent_id.uuid,
            fingerprint: promise.fingerprint,
            oplog_idx: promise.oplog_idx,
        }
    }
}

impl From<&StoredPromise> for PromiseId {
    fn from(promise: &StoredPromise) -> Self {
        PromiseId {
            agent_id: AgentId::new(promise.agent_id),
            fingerprint: promise.fingerprint,
            oplog_idx: promise.oplog_idx,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct HookState {
    effect: String,
    port: u16,
    value: u64,
    promise: Option<StoredPromise>,
    /// The ID of the agent that saved the snapshot.
    #[serde(default)]
    saved_by: String,
    #[serde(skip)]
    restored: bool,
}

#[agent_definition(snapshotting = "enabled")]
pub trait HookEffect {
    fn new(effect: String, port: u16) -> Self;
    fn set(&mut self, value: u64) -> u64;
    fn value(&self) -> u64;
    /// Creates a promise that the load hook of the effect `complete-promise` completes.
    fn prepare_promise(&mut self) -> PromiseId;
    /// Panics in a restored agent: a failure of the target code after a load.
    fn fail_if_restored(&self) -> u64;
    /// Sends an HTTP request to the peer at `port` and returns the response status.
    async fn call_http(&self) -> u64;
    /// Calls `wait_forever` on the peer agent `HookEffect("rpc-peer", port)`.
    async fn call_rpc_wait(&self);
    /// Awaits a promise that nothing completes.
    fn wait_forever(&self);
}

pub struct HookEffectImpl {
    state: HookState,
}

fn loopback(port: u16) -> IpSocketAddress {
    IpSocketAddress::Ipv4(Ipv4SocketAddress {
        address: (127, 0, 0, 1),
        port,
    })
}

async fn http_get(port: u16) -> Result<(), String> {
    let request = golem_rust::http::Request::builder()
        .uri(format!("http://127.0.0.1:{port}/hook"))
        .body(Vec::new())
        .map_err(|error| error.to_string())?;
    golem_rust::http::send(request)
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

/// Calls the peer agent `HookEffect("rpc-peer", port)`, so each test has its own peer.
async fn rpc_get(port: u16) -> u64 {
    HookEffectClient::upsert("rpc-peer".to_string(), port)
        .value()
        .await
}

/// The agent's own ID, from its metadata.
fn own_agent_id() -> Result<String, String> {
    Ok(golem_rust::get_self_metadata()
        .map_err(|error| format!("self metadata: {error:?}"))?
        .agent_id
        .uuid
        .to_string())
}

impl HookEffectImpl {
    async fn load_effect(state: &mut HookState) -> Result<(), String> {
        let port = state.port;
        match state.effect.as_str() {
            "http" => http_get(port).await?,
            "rpc" => {
                rpc_get(port).await;
            }
            "create-promise" => {
                golem_rust::create_promise();
            }
            "complete-promise" => {
                let promise = state
                    .promise
                    .as_ref()
                    .ok_or("no promise to complete")?
                    .into();
                golem_rust::complete_promise(&promise, b"from the load hook");
            }
            "card" => {
                permissions::derive_from_wallet(&Bounds::default(), None)
                    .map_err(|error| format!("{error:?}"))?;
            }
            "tcp" => {
                let socket = TcpSocket::create(IpAddressFamily::Ipv4)
                    .map_err(|error| format!("create: {error:?}"))?;
                socket
                    .connect(loopback(port))
                    .await
                    .map_err(|error| format!("connect: {error:?}"))?;
            }
            "udp" => {
                let socket = UdpSocket::create(IpAddressFamily::Ipv4)
                    .map_err(|error| format!("create: {error:?}"))?;
                socket
                    .send(b"from the load hook".to_vec(), Some(loopback(port)))
                    .await
                    .map_err(|error| format!("send: {error:?}"))?;
            }
            "udp-receive" => {
                let socket = UdpSocket::create(IpAddressFamily::Ipv4)
                    .map_err(|error| format!("create: {error:?}"))?;
                socket
                    .bind(loopback(port))
                    .map_err(|error| format!("{error:?}"))?;
                socket
                    .receive()
                    .await
                    .map_err(|error| format!("{error:?}"))?;
            }
            "random-in-task" => {
                wit_bindgen::spawn_local(async {
                    golem_rust::wasip3::random::random::get_random_bytes(8);
                })
                .await
                .ok_or("the spawned task was cancelled")?;
            }
            "rdbms-begin" => {
                let connection = golem_rust::bindings::golem::rdbms::postgres::DbConnection::open(
                    &format!("postgres://postgres:postgres@localhost:{port}/postgres"),
                )
                .map_err(|error| format!("open: {error:?}"))?;
                let transaction = connection
                    .begin_transaction()
                    .map_err(|error| format!("begin: {error:?}"))?;
                transaction
                    .rollback()
                    .map_err(|error| format!("rollback: {error:?}"))?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[agent_implementation]
impl HookEffect for HookEffectImpl {
    fn new(effect: String, port: u16) -> Self {
        Self {
            state: HookState {
                effect,
                port,
                ..HookState::default()
            },
        }
    }

    fn set(&mut self, value: u64) -> u64 {
        self.state.value = value;
        value
    }

    fn value(&self) -> u64 {
        self.state.value
    }

    fn prepare_promise(&mut self) -> PromiseId {
        let promise = golem_rust::create_promise();
        self.state.promise = Some((&promise).into());
        promise
    }

    fn fail_if_restored(&self) -> u64 {
        assert!(!self.state.restored, "the restored agent fails");
        self.state.value
    }

    async fn call_http(&self) -> u64 {
        match http_get(self.state.port).await {
            Ok(()) => 200,
            Err(_) => 0,
        }
    }

    async fn call_rpc_wait(&self) {
        HookEffectClient::upsert("rpc-peer".to_string(), self.state.port)
            .wait_forever()
            .await;
    }

    fn wait_forever(&self) {
        golem_rust::blocking_await_promise(&golem_rust::create_promise());
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        let mut state = serde_json::to_value(&self.state).map_err(|error| error.to_string())?;
        state["saved_by"] = own_agent_id()?.into();
        serde_json::to_vec(&state).map_err(|error| error.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let mut state: HookState =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        state.restored = true;
        if state.saved_by != own_agent_id()? {
            Self::load_effect(&mut state).await?;
        }
        Ok(Self { state })
    }
}
