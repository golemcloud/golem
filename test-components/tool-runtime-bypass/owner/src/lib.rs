use futures_concurrency::prelude::*;
use golem_rust::agentic::{Config, Secret, spawn_local};
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
    /// Announces `ambient-starting`, starts `permission-probe read <path>` through an
    /// asynchronously lowered `invoke-and-await`, and returns `returned-early` while that call is
    /// still being admitted, without awaiting it.
    async fn start_permission_probe_and_return(&self, path: String) -> String;
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

    async fn start_permission_probe_and_return(&self, path: String) -> String {
        let rpc = tool_host::ToolRpc::create("permission-probe").expect("bind permission-probe");
        let input = golem_rust::encode_typed_schema_value(
            &PermissionProbeInput { path }
                .into_typed_schema_value()
                .expect("permission-probe input schema"),
        )
        .expect("encode permission-probe input");
        announce("ambient-starting").await;
        let mut call = Box::pin(async move {
            rpc.invoke_and_await(vec!["read".to_string()], input, None, None, None)
                .await
        });
        let pending = std::future::poll_fn(|context| {
            std::task::Poll::Ready(call.as_mut().poll(context).is_pending())
        })
        .await;
        if !pending {
            return "completed-before-return".to_string();
        }
        spawn_local(async move {
            let _ = call.await;
        });
        "returned-early".to_string()
    }
}

async fn announce(path: &str) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("EFFECT_PORT").expect("EFFECT_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid effect fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set authority");
    request
        .set_path_with_query(Some(&format!("/{path}")))
        .expect("set path");
    let send = async move { client::send(request).await.expect("send effect") };
    let finish = async move {
        trailers_tx.write(Ok(None)).await.expect("finish trailers");
        transmit.await.expect("transmit effect");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
}
