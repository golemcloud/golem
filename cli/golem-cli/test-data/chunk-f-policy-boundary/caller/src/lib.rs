use chunk_f_config_probe_tool_guest_client::ChunkFConfigProbeClient;
use chunk_f_filesystem_probe_tool_guest_client::ChunkFFilesystemProbeClient;
use golem_rust::agentic::Config;
use golem_rust::{ConfigSchema, agent_definition, agent_implementation};

#[derive(ConfigSchema)]
pub struct ChunkFPolicyConfig {
    pub allowed: Option<String>,
    pub denied: Option<String>,
    pub outside: Option<String>,
}

#[agent_definition]
pub trait ChunkFPolicyAgent {
    fn new(name: String, #[agent_config] config: Config<ChunkFPolicyConfig>) -> Self;
    async fn read_config(&self, key: String) -> String;
    async fn filesystem_roundtrip(&self, path: String, value: String) -> String;
}

struct ChunkFPolicyAgentImpl;

#[agent_implementation]
impl ChunkFPolicyAgent for ChunkFPolicyAgentImpl {
    fn new(_name: String, #[agent_config] _config: Config<ChunkFPolicyConfig>) -> Self {
        Self
    }

    async fn read_config(&self, key: String) -> String {
        ChunkFConfigProbeClient::new().read(key).await.unwrap()
    }

    async fn filesystem_roundtrip(&self, path: String, value: String) -> String {
        ChunkFFilesystemProbeClient::new()
            .roundtrip(path, value)
            .await
            .unwrap()
    }
}
