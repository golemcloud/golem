use golem_rust::agentic::{Config, Secret};
use golem_rust::{
    ConfigSchema, FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, agent_definition,
    agent_implementation,
};
use validation_probe_tool_guest_client::ValidationProbeClient;

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ValidationEvidence {
    pub label: String,
    pub ordinal: u64,
    pub stdout: Vec<u8>,
}

#[derive(ConfigSchema)]
pub struct ToolRuntimeBypassOwnerConfig {
    #[config_schema(secret)]
    pub context_secret: Secret<String>,
}

#[agent_definition]
pub trait ToolRuntimeBypassOwner {
    fn new(name: String, #[agent_config] config: Config<ToolRuntimeBypassOwnerConfig>) -> Self;
    async fn valid_transformation(&self) -> ValidationEvidence;
}

struct ToolRuntimeBypassOwnerImpl;

#[agent_implementation]
impl ToolRuntimeBypassOwner for ToolRuntimeBypassOwnerImpl {
    fn new(
        _name: String,
        #[agent_config] _config: Config<ToolRuntimeBypassOwnerConfig>,
    ) -> Self {
        Self
    }

    async fn valid_transformation(&self) -> ValidationEvidence {
        let mut invocation = ValidationProbeClient::default()
            .validate("valid-transform".to_string())
            .await
            .expect("start validation probe");
        let result = invocation.result().await.expect("validation succeeds");
        let mut stdout = Vec::new();
        let mut stream = invocation.stdout.take().expect("declared stdout");
        while let Some(chunk) = stream.next().await {
            stdout.extend(chunk.expect("validation stdout succeeds"));
        }
        ValidationEvidence {
            label: result.nested.label,
            ordinal: result.nested.ordinal,
            stdout,
        }
    }
}
