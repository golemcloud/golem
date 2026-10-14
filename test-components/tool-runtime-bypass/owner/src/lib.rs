use golem_rust::agentic::{Config, Secret};
use golem_rust::bindings::golem::tool::host as tool_host;
use golem_rust::schema::IntoTypedSchemaValue;
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

#[derive(IntoSchema)]
struct PermissionProbeInput {
    path: String,
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
    /// Calls `permission-probe read /wait-delete/public`, whose middleware performs an Underlying
    /// `delete /public` after a promise checkpoint, and then an ambient `delete /public`.
    /// Returns both outcomes as `underlying=<outcome>;ambient=<outcome>`.
    async fn underlying_then_ambient_delete(&self) -> String;
}

struct ToolRuntimeBypassOwnerImpl;

#[agent_implementation]
impl ToolRuntimeBypassOwner for ToolRuntimeBypassOwnerImpl {
    fn new(_name: String, #[agent_config] _config: Config<ToolRuntimeBypassOwnerConfig>) -> Self {
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

    async fn underlying_then_ambient_delete(&self) -> String {
        let underlying = permission_probe_outcome("read", "/wait-delete/public").await;
        let ambient = permission_probe_outcome("delete", "/public").await;
        format!("underlying={underlying};ambient={ambient}")
    }
}

/// Invokes `permission-probe <command> <path>` and describes the outcome as the middleware does:
/// the middleware result on success, `denied` when the ambient admission is denied.
async fn permission_probe_outcome(command: &str, path: &str) -> String {
    let rpc = tool_host::ToolRpc::create("permission-probe").expect("bind permission-probe");
    let input = golem_rust::encode_typed_schema_value(
        &PermissionProbeInput {
            path: path.to_string(),
        }
        .into_typed_schema_value()
        .expect("permission-probe input schema"),
    )
    .expect("encode permission-probe input");
    match rpc
        .invoke_and_await(vec![command.to_string()], input, None, None, None)
        .await
    {
        Ok(result) => match result
            .result
            .as_ref()
            .map(|value| golem_rust::decode_typed_schema_value(value))
        {
            Some(Ok(value)) => match value.value() {
                golem_rust::SchemaValue::String(outcome) => outcome.clone(),
                other => format!("unexpected:{other:?}"),
            },
            other => format!("unexpected:{other:?}"),
        },
        Err(tool_host::ToolRpcError::Denied(_)) => "denied".to_string(),
        Err(error) => format!("error:{error:?}"),
    }
}
