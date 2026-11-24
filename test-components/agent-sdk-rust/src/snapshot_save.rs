use crate::readonly::agent::ReadonlyAgentClient;
use golem_rust::{agent_definition, agent_implementation, create_promise};

/// An agent whose save hook makes the side effect that `mode` names: "write" creates a promise,
/// "http" sends a GET request to the URL `peer`, and "rpc" increments the `ReadonlyAgent` whose
/// key is `peer`. The executor runs the save hook read-only, so it must refuse each of them.
#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotSaveProbe {
    fn new(mode: String, peer: String) -> Self;

    fn increment(&mut self) -> u64;

    fn get(&self) -> u64;
}

struct SnapshotSaveProbeImpl {
    value: u64,
    mode: String,
    peer: String,
}

#[agent_implementation]
impl SnapshotSaveProbe for SnapshotSaveProbeImpl {
    fn new(mode: String, peer: String) -> Self {
        Self {
            value: 0,
            mode,
            peer,
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
            "write" => {
                let _ = create_promise();
            }
            "http" => {
                let _ = wasi_fetch::Client::new().get(&self.peer).send().await;
            }
            "rpc" => {
                let _ = ReadonlyAgentClient::upsert(self.peer.clone())
                    .increment()
                    .await;
            }
            other => return Err(format!("unsupported snapshot save mode: {other}")),
        }
        Ok(self.value.to_be_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let bytes: [u8; 8] = bytes
            .try_into()
            .map_err(|bytes: Vec<u8>| format!("invalid snapshot size: {}", bytes.len()))?;
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let mode = parameters.take::<String>().map_err(|e| e.to_string())?;
        let peer = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self {
            value: u64::from_be_bytes(bytes),
            mode,
            peer,
        })
    }
}
