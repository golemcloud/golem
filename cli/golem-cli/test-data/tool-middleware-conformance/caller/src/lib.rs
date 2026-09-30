use golem_rust::{agent_definition, agent_implementation};
use manifest_probe_tool_guest_client::ManifestProbeClient;
use compat_leaf_tool_guest_client::{CompatInput, CompatLeafClient};

#[agent_definition]
pub trait MiddlewareConformanceAgent {
    fn new(name: String) -> Self;
    async fn invoke(&self, value: String) -> String;
    fn effects(&self) -> String;
    fn clear_effects(&self);
}

struct MiddlewareConformanceAgentImpl;

#[agent_implementation]
impl MiddlewareConformanceAgent for MiddlewareConformanceAgentImpl {
    fn new(_name: String) -> Self {
        Self
    }

    async fn invoke(&self, value: String) -> String {
        match ManifestProbeClient::new().apply(value).await {
            Ok(value) => format!("ok:{value}"),
            Err(error) => format!("err:{error:?}"),
        }
    }

    fn effects(&self) -> String {
        std::fs::read_to_string("/chunk-d-effects.log").unwrap_or_default()
    }

    fn clear_effects(&self) {
        let _ = std::fs::remove_file("/chunk-d-effects.log");
    }
}

#[agent_definition]
pub trait AdapterConformanceAgent {
    fn new(name: String) -> Self;
    async fn invoke(&self, kept: String) -> String;
}

struct AdapterConformanceAgentImpl;

#[agent_implementation]
impl AdapterConformanceAgent for AdapterConformanceAgentImpl {
    fn new(_name: String) -> Self {
        Self
    }

    async fn invoke(&self, kept: String) -> String {
        match CompatLeafClient::new().execute(CompatInput { kept }).await {
            Ok(output) => format!("ok:{}", output.kept),
            Err(error) => format!("err:{error:?}"),
        }
    }
}
