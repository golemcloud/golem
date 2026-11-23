use crate::readonly::agent::ReadonlyAgentClient;
use golem_rust::agentic::{AgentStream, SnapshotRestoreContext, spawn_local};
use golem_rust::{agent_definition, agent_implementation, read_only};

/// The peer of a streaming RPC call: each call of `count_up` counts and streams the new count.
#[agent_definition]
pub trait SnapshotSaveStreamPeer {
    fn new(id: String) -> Self;

    fn count_up(&mut self) -> AgentStream<u64>;

    fn get_count(&self) -> u64;
}

struct SnapshotSaveStreamPeerImpl {
    count: u64,
}

#[agent_implementation]
impl SnapshotSaveStreamPeer for SnapshotSaveStreamPeerImpl {
    fn new(_id: String) -> Self {
        Self { count: 0 }
    }

    fn count_up(&mut self) -> AgentStream<u64> {
        self.count += 1;
        let count = self.count;
        let (mut writer, stream) = AgentStream::new();
        spawn_local(async move {
            let _ = writer.write_all(vec![count]).await;
            let _ = writer.close().await;
        });
        stream
    }

    fn get_count(&self) -> u64 {
        self.count
    }
}

/// An agent whose save hook makes the outgoing call that `mode` names: `rpc` increments the
/// `ReadonlyAgent` named `target`, `rpc-stream` reads the stream of `count_up` from the
/// `SnapshotSaveStreamPeer` named `target`, `http-post` posts to the URL `target` and reads the
/// response, and `http-get` gets the URL `target` and drops the response unread. A failed call
/// fails the save.
#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotSaveProbe {
    fn new(mode: String, target: String) -> Self;

    fn increment(&mut self) -> u64;

    /// Read-only, so that it does not make a periodic snapshot due.
    #[read_only]
    fn get(&self) -> u64;
}

struct SnapshotSaveProbeImpl {
    value: u64,
    mode: String,
    target: String,
}

#[agent_implementation]
impl SnapshotSaveProbe for SnapshotSaveProbeImpl {
    fn new(mode: String, target: String) -> Self {
        Self {
            value: 0,
            mode,
            target,
        }
    }

    fn increment(&mut self) -> u64 {
        self.value += 1;
        self.value
    }

    fn get(&self) -> u64 {
        self.value
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        match self.mode.as_str() {
            "rpc" => {
                ReadonlyAgentClient::upsert(self.target.clone())
                    .increment()
                    .await;
            }
            "rpc-stream" => {
                let values = SnapshotSaveStreamPeerClient::upsert(self.target.clone())
                    .count_up()
                    .await
                    .collect()
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                if values.len() != 1 {
                    return Err(format!("unexpected stream items {values:?}"));
                }
            }
            "http-post" => {
                let request = golem_rust::http::Request::post(self.target.as_str())
                    .body(Vec::new())
                    .map_err(|error| error.to_string())?;
                let response = golem_rust::http::send(request)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                if !response.status().is_success() {
                    return Err(format!("unexpected status {}", response.status()));
                }
                golem_rust::http::response_bytes(response)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
            }
            "http-get" => {
                // The response is dropped with its body unread.
                let request = golem_rust::http::Request::get(self.target.as_str())
                    .body(Vec::new())
                    .map_err(|error| error.to_string())?;
                golem_rust::http::send(request)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
            }
            other => return Err(format!("unsupported snapshot save mode: {other}")),
        }
        Ok(self.value.to_be_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let bytes: [u8; 8] = bytes
            .try_into()
            .map_err(|bytes: Vec<u8>| format!("invalid snapshot size: {}", bytes.len()))?;
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let mode = parameters.take::<String>().map_err(|e| e.to_string())?;
        let target = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self {
            value: u64::from_be_bytes(bytes),
            mode,
            target,
        })
    }
}
