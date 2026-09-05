use crate::readonly::agent::ReadonlyAgentClient;
use golem_rust::agentic::{Config, SnapshotRestoreContext};
use golem_rust::{
    ConfigSchema, SchemaValue, agent_definition, agent_implementation, create_promise,
};
use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};

static CONSTRUCTOR_CALLS: AtomicU32 = AtomicU32::new(0);
static LOAD_CALLS: AtomicU32 = AtomicU32::new(0);

#[derive(ConfigSchema)]
pub struct SnapshotLoadProbeConfig {
    pub marker: String,
}

#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotLoadProbe {
    fn new(mode: String, #[agent_config] config: Config<SnapshotLoadProbeConfig>) -> Self;

    fn increment(&mut self) -> u64;

    fn status(&self) -> String;
}

struct SnapshotLoadProbeImpl {
    value: u64,
    loaded_value: Option<u64>,
    origin: &'static str,
    mode: String,
    principal: String,
    agent_type: String,
    phantom_id: Option<String>,
    config_marker: String,
    read_bytes: usize,
    constructor_calls_at_restore: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotLoadProbeStatus<'a> {
    value: u64,
    loaded_value: Option<u64>,
    origin: &'a str,
    mode: &'a str,
    principal: &'a str,
    agent_type: &'a str,
    phantom_id: &'a Option<String>,
    config_marker: &'a str,
    read_bytes: usize,
    constructor_calls_at_restore: u32,
    constructor_calls_now: u32,
    load_calls_now: u32,
}

#[agent_implementation]
impl SnapshotLoadProbe for SnapshotLoadProbeImpl {
    fn new(mode: String, #[agent_config] config: Config<SnapshotLoadProbeConfig>) -> Self {
        CONSTRUCTOR_CALLS.fetch_add(1, Ordering::SeqCst);
        Self {
            value: 0,
            loaded_value: None,
            origin: "initialized",
            mode,
            principal: "initialized".to_string(),
            agent_type: "SnapshotLoadProbe".to_string(),
            phantom_id: None,
            config_marker: config
                .get()
                .expect("config access should be allowed")
                .marker,
            read_bytes: 0,
            constructor_calls_at_restore: 0,
        }
    }

    fn increment(&mut self) -> u64 {
        self.value += 1;
        self.value
    }

    fn status(&self) -> String {
        serde_json::to_string(&SnapshotLoadProbeStatus {
            value: self.value,
            loaded_value: self.loaded_value,
            origin: self.origin,
            mode: &self.mode,
            principal: &self.principal,
            agent_type: &self.agent_type,
            phantom_id: &self.phantom_id,
            config_marker: &self.config_marker,
            read_bytes: self.read_bytes,
            constructor_calls_at_restore: self.constructor_calls_at_restore,
            constructor_calls_now: CONSTRUCTOR_CALLS.load(Ordering::SeqCst),
            load_calls_now: LOAD_CALLS.load(Ordering::SeqCst),
        })
        .expect("snapshot load probe status should serialize")
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.value.to_be_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        LOAD_CALLS.fetch_add(1, Ordering::SeqCst);
        let constructor_calls_at_restore = CONSTRUCTOR_CALLS.load(Ordering::SeqCst);

        let bytes: [u8; 8] = bytes
            .try_into()
            .map_err(|bytes: Vec<u8>| format!("invalid snapshot size: {}", bytes.len()))?;
        let value = u64::from_be_bytes(bytes);

        let SchemaValue::Record { fields } = context.parameters else {
            return Err("invalid snapshot restore parameters".to_string());
        };
        let [SchemaValue::String(mode)] = fields.as_slice() else {
            return Err("invalid snapshot restore parameters".to_string());
        };
        let mode = mode.clone();

        let config_marker = Config::<SnapshotLoadProbeConfig>::new()
            .get()
            .map_err(|error| format!("failed to read config: {error:?}"))?
            .marker;
        let read_bytes = golem_rust::wasip3::random::random::get_random_bytes(4).len();

        if value > 1 {
            match mode.as_str() {
                "read" => {}
                "write" => {
                    let _ = create_promise();
                }
                "http" => {
                    let _ = wasi_fetch::Client::new()
                        .get("http://127.0.0.1:1/")
                        .send()
                        .await;
                }
                "rpc" => {
                    let client =
                        ReadonlyAgentClient::get("snapshot-load-probe-rpc-target".to_string());
                    let _ = client.get_count().await;
                }
                other => return Err(format!("unsupported snapshot load mode: {other}")),
            }
        }

        Ok(Self {
            value,
            loaded_value: Some(value),
            origin: "restored",
            mode,
            principal: serde_json::to_string(&context.principal)
                .expect("principal should serialize"),
            agent_type: context.agent_type,
            phantom_id: context.phantom_id.map(|id| id.to_string()),
            config_marker,
            read_bytes,
            constructor_calls_at_restore,
        })
    }
}
