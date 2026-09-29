use futures_concurrency::prelude::*;
use golem_rust::agentic::{AgentStream, Secret, spawn_local};
use golem_rust::schema::tool::{ErrorCase, ErrorKind};
use golem_rust::schema::try_into_schema_graph;
use golem_rust::secrets::GuestSecretHandle;
use golem_rust::tool::{
    EmptyMiddlewareParameters, InputStream, InvocationResult, MonomorphicToolMiddlewareScope,
    OutputStream, Principal, RawCustomToolError, Tool, ToolInvokeError, ToolMiddleware,
    ToolMiddlewareInvokeFuture, ToolMiddlewareScope, ToolUnderlying, UnderlyingTool,
};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoTypedSchemaValue, IntoWire, SchemaType, SchemaValue,
    TypedSchemaValue, WireSchema, decode_schema_value, encode_schema_graph, tool_definition,
    tool_middleware, universal_tool_middleware,
};
use std::convert::Infallible;

#[tool_definition(version = "1.0.0")]
pub trait MiddlewareProbe {
    async fn apply(&self, value: String) -> String;
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct SecretPolicyObservation {
    pub label: String,
    pub config_resolved: bool,
    pub configured_secret_revealed: bool,
    pub input_secret_revealed: bool,
}

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct SecretPolicyEvidence {
    pub middleware: Vec<SecretPolicyObservation>,
    pub leaf_revealed: bool,
}

#[tool_definition(version = "1.0.0")]
pub trait SecretPolicyProbe {
    async fn inspect(&self, value: GuestSecretHandle) -> SecretPolicyEvidence;
}

#[derive(IntoSchema, FromSchema)]
struct SecretPolicyParameters {
    label: String,
}

struct SecretPolicyAudit {
    parameters: SecretPolicyParameters,
}

impl SecretPolicyAudit {
    fn new(parameters: SecretPolicyParameters) -> Self {
        Self { parameters }
    }
}

fn reveal_string(value: &GuestSecretHandle) -> Result<String, String> {
    let graph =
        golem_rust::schema::try_into_schema_graph::<String>().map_err(|error| error.to_string())?;
    let expected = encode_schema_graph(&graph).map_err(|error| error.to_string())?;
    let value = value
        .with_handle(|handle| {
            golem_rust::bindings::golem::secrets::reveal::reveal(handle, &expected)
        })
        .ok_or_else(|| "secret handle was transferred".to_string())?
        .map_err(|error| format!("{error:?}"))?;
    let value = decode_schema_value(value).map_err(|error| error.to_string())?;
    String::from_value(&value).map_err(|error| error.to_string())
}

#[tool_middleware(
    name = "streaming-secret-policy-audit",
    constructor = SecretPolicyAudit::new,
    parameters = SecretPolicyParameters
)]
impl SecretPolicyProbeMiddleware for SecretPolicyAudit {
    async fn inspect(
        &self,
        underlying: &SecretPolicyProbeUnderlying,
        value: GuestSecretHandle,
    ) -> Result<SecretPolicyEvidence, ToolInvokeError<Infallible>> {
        if self.parameters.label == "restricted"
            && std::env::var("MIDDLEWARE_PROMISE_CHECKPOINT_PORT").is_ok()
        {
            wait_at_middleware_promise_checkpoint("secret-policy-before-access").await;
        }
        let configured = Secret::<String>::new(vec!["tool_secret".to_string()]).handle();
        let config_resolved = configured.is_ok();
        let configured_secret_revealed = configured
            .as_ref()
            .is_ok_and(|configured| reveal_string(configured).is_ok());
        let input_secret_revealed = reveal_string(&value).is_ok();
        let mut evidence = underlying.inspect(value).await?;
        if self.parameters.label == "restricted"
            && std::env::var("MIDDLEWARE_PROMISE_CHECKPOINT_PORT").is_ok()
        {
            wait_at_middleware_promise_checkpoint("secret-policy-after-forward").await;
        }
        evidence.middleware.push(SecretPolicyObservation {
            label: self.parameters.label.clone(),
            config_resolved,
            configured_secret_revealed,
            input_secret_revealed,
        });
        Ok(evidence)
    }
}

mod expected_mcp_probe {
    use super::*;

    #[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    pub struct Evidence {
        pub evidence: String,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait MiddlewareProbe {
        async fn middleware_probe(&self, value: String, stdout: Option<OutputStream>) -> Evidence;
    }
}

mod presented_mcp_probe {
    pub use super::expected_mcp_probe::Evidence;
    use super::*;

    #[tool_definition(version = "1.0.0")]
    pub trait MiddlewareProbe {
        async fn middleware_probe(&self, value: String, stdout: Option<OutputStream>) -> Evidence;
    }
}

struct McpProbeProjection;

impl McpProbeProjection {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "streaming-monomorphic-mcp-projection-typed", constructor = McpProbeProjection::new)]
impl presented_mcp_probe::MiddlewareProbeMiddleware<expected_mcp_probe::MiddlewareProbeUnderlying>
    for McpProbeProjection
{
    async fn middleware_probe(
        &self,
        underlying: &expected_mcp_probe::MiddlewareProbeUnderlying,
        value: String,
        mut stdout: Option<OutputStream>,
    ) -> Result<presented_mcp_probe::Evidence, ToolInvokeError<Infallible>> {
        let (result, mut underlying_stdout) = underlying.middleware_probe(value).await?;
        while let Some(item) = underlying_stdout.next().await {
            match item {
                Ok(bytes) => {
                    if let Some(stdout) = &mut stdout {
                        let _ = stdout.write(bytes).await;
                    }
                }
                Err(failure) => {
                    if let Some(stdout) = stdout.take() {
                        let _ = stdout.fail(failure).await;
                    }
                    break;
                }
            }
        }
        if let Some(stdout) = stdout {
            let _ = stdout.finish().await;
        }
        Ok(presented_mcp_probe::Evidence {
            evidence: format!("monomorphic({})", result.evidence),
        })
    }
}

fn invoke_mcp_projection(
    _tool_name: String,
    _tool_metadata: Tool,
    _parameters: TypedSchemaValue,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> ToolMiddlewareInvokeFuture {
    Box::pin(async move {
        let (graph, mut value) = input.into_parts();
        let SchemaValue::Record { fields } = &mut value else {
            panic!("MCP input is a record");
        };
        let SchemaValue::String(argument) = &mut fields[0] else {
            panic!("MCP value argument is a string");
        };
        *argument = format!("monomorphic({argument})");
        let mut completed = underlying
            .invoke_forwarding_stdout(
                command_path,
                TypedSchemaValue::new(graph, value),
                stdin,
                stdout,
            )
            .await?;
        completed.result = None;
        Ok(completed)
    })
}

golem_rust::ctor::__support::ctor_parse!(
    #[ctor]
    fn register_mcp_projection() {
        let mut presented =
            <McpProbeProjection as presented_mcp_probe::MiddlewareProbeMiddleware<
                expected_mcp_probe::MiddlewareProbeUnderlying,
            >>::__golem_presented_tool_descriptor();
        let mut expected = <expected_mcp_probe::MiddlewareProbeUnderlying as ToolUnderlying>::__golem_tool_descriptor();
        for descriptor in [&mut presented, &mut expected] {
            let body = descriptor.commands.nodes[0].body.as_mut().unwrap();
            body.stdout.as_mut().unwrap().mime = vec!["*/*".to_string()];
        }
        presented.commands.nodes[0].body.as_mut().unwrap().result = None;
        let expected_body = expected.commands.nodes[0].body.as_mut().unwrap();
        expected_body.result.as_mut().unwrap().type_ = SchemaType::record(Vec::new());
        expected_body.errors.push(ErrorCase {
            name: "mcp-tool-error".to_string(),
            doc: Default::default(),
            kind: ErrorKind::RuntimeError,
            exit_code: 1,
            payload: Some(SchemaType::string()),
        });
        golem_rust::tool::register_tool_middleware(
            ToolMiddleware {
                name: "streaming-monomorphic-mcp-projection".to_string(),
                version: "0.0.0".to_string(),
                aliases: Vec::new(),
                doc: Default::default(),
                scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
                    presented,
                    expected: Some(expected),
                })),
                parameter_schema: try_into_schema_graph::<EmptyMiddlewareParameters>().unwrap(),
            },
            invoke_mcp_projection,
        );
    }
);

mod expected_typed_output {
    use super::*;

    #[derive(IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
    pub struct Item {
        pub ordinal: u32,
        pub label: String,
        pub asymmetric_extra: u64,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait TypedOutputStream {
        async fn produce(&self, tag: String) -> AgentStream<Item>;
    }
}

mod presented_typed_output {
    use super::*;

    #[derive(IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
    pub struct Item {
        pub label: String,
        pub ordinal: u32,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait TypedOutputStream {
        async fn produce(&self, tag: String) -> AgentStream<Item>;
    }
}

struct TypedOutputProjection;

impl TypedOutputProjection {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "streaming-typed-output-projection", constructor = TypedOutputProjection::new)]
impl
    presented_typed_output::TypedOutputStreamMiddleware<
        expected_typed_output::TypedOutputStreamUnderlying,
    > for TypedOutputProjection
{
    async fn produce(
        &self,
        underlying: &expected_typed_output::TypedOutputStreamUnderlying,
        tag: String,
    ) -> Result<AgentStream<presented_typed_output::Item>, ToolInvokeError<Infallible>> {
        let mut source = underlying.produce(tag).await?;
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Some(item) = source.next().await.expect("read provider typed output") {
                writer
                    .write_one(presented_typed_output::Item {
                        label: item.label,
                        ordinal: item.ordinal,
                    })
                    .await
                    .expect("write projected typed output");
            }
        });
        Ok(output)
    }
}

mod expected_typed_input {
    use super::*;

    #[derive(IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
    pub struct Item {
        pub ordinal: u32,
        pub caller_extra: u64,
        pub label: String,
    }

    #[derive(IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
    pub struct Evidence {
        pub label: String,
        pub ordinal: u32,
    }

    #[tool_definition(version = "1.0.0")]
    pub trait TypedInputStream {
        async fn consume(&self, input: AgentStream<Item>) -> Vec<Evidence>;
    }
}

mod presented_typed_input {
    pub use super::expected_typed_input::{Evidence, Item};
    use super::*;

    #[tool_definition(version = "1.0.0")]
    pub trait TypedInputStream {
        async fn consume(&self, input: AgentStream<Item>) -> Vec<Evidence>;
    }
}

struct TypedInputProjection;

impl TypedInputProjection {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "streaming-typed-input-projection", constructor = TypedInputProjection::new)]
impl
    presented_typed_input::TypedInputStreamMiddleware<
        expected_typed_input::TypedInputStreamUnderlying,
    > for TypedInputProjection
{
    async fn consume(
        &self,
        underlying: &expected_typed_input::TypedInputStreamUnderlying,
        input: AgentStream<presented_typed_input::Item>,
    ) -> Result<Vec<presented_typed_input::Evidence>, ToolInvokeError<Infallible>> {
        underlying.consume(input).await
    }
}

#[universal_tool_middleware(name = "streaming-universal-pass-through")]
async fn universal_pass_through(
    _tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    underlying
        .invoke_forwarding_stdout(command_path, input, stdin, stdout)
        .await
}

#[universal_tool_middleware(name = "streaming-universal-secret-policy-audit")]
async fn universal_secret_policy_audit(
    tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    if tool_name != "secret-policy-probe" {
        return underlying
            .invoke_forwarding_stdout(command_path, input, stdin, stdout)
            .await;
    }

    let configured = Secret::<String>::new(vec!["tool_secret".to_string()]).handle();
    let config_resolved = configured.is_ok();
    let configured_secret_revealed = configured
        .as_ref()
        .is_ok_and(|configured| reveal_string(configured).is_ok());
    let input_secret_revealed = match input.value() {
        SchemaValue::Record { fields } => fields.first().is_some_and(
            |value| matches!(value, SchemaValue::Secret(handle) if reveal_string(handle).is_ok()),
        ),
        _ => false,
    };
    let mut result = underlying
        .invoke_forwarding_stdout(command_path, input, stdin, stdout)
        .await?;
    let value = result.result.take().ok_or_else(|| {
        ToolInvokeError::InvalidResult("secret policy probe returned no value".to_string())
    })?;
    let mut evidence = SecretPolicyEvidence::from_value(value.value())
        .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
    evidence.middleware.push(SecretPolicyObservation {
        label: "universal".to_string(),
        config_resolved,
        configured_secret_revealed,
        input_secret_revealed,
    });
    result.result = Some(
        evidence
            .into_typed_schema_value()
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
    );
    Ok(result)
}

#[universal_tool_middleware(name = "streaming-universal-transform-input")]
async fn universal_transform_input(
    _tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let (graph, mut value) = input.into_parts();
    let SchemaValue::Record { fields } = &mut value else {
        panic!("MCP input is a record");
    };
    let SchemaValue::String(argument) = &mut fields[0] else {
        panic!("MCP value argument is a string");
    };
    *argument = format!("middleware({argument})");
    underlying
        .invoke_forwarding_stdout(
            command_path,
            TypedSchemaValue::new(graph, value),
            stdin,
            stdout,
        )
        .await
}

#[universal_tool_middleware(name = "streaming-universal-mcp-fanout")]
async fn universal_mcp_fanout(
    _tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    fn with_suffix(input: &TypedSchemaValue, suffix: &str) -> TypedSchemaValue {
        let (graph, mut value) = input.clone().into_parts();
        let SchemaValue::Record { fields } = &mut value else {
            panic!("MCP input is a record");
        };
        let SchemaValue::String(argument) = &mut fields[0] else {
            panic!("MCP value argument is a string");
        };
        *argument = format!("{argument}-{suffix}");
        TypedSchemaValue::new(graph, value)
    }

    underlying
        .invoke(command_path.clone(), with_suffix(&input, "first"), None)
        .await?;
    let completed = underlying
        .invoke_forwarding_stdout(
            command_path.clone(),
            with_suffix(&input, "second"),
            stdin,
            stdout,
        )
        .await?;
    let pending = underlying
        .start(command_path, with_suffix(&input, "pending"), None)
        .await?;
    wait_at_middleware_promise_checkpoint("mcp-pending-admitted").await;
    pending.cancel();
    Ok(completed)
}

macro_rules! streaming_middleware {
    ($type:ident, [$($attribute:tt)*], |$underlying:ident, $value:ident| $body:block) => {
        struct $type;

        impl $type {
            fn new() -> Self {
                Self
            }
        }

        #[tool_middleware($($attribute)*, constructor = $type::new)]
        impl MiddlewareProbeMiddleware for $type {
            async fn apply(
                &self,
                $underlying: &MiddlewareProbeUnderlying,
                $value: String,
            ) -> Result<String, ToolInvokeError<Infallible>> $body
        }
    };
}

streaming_middleware!(
    Transform,
    [name = "streaming-transform"],
    |underlying, value| {
        let result = underlying.apply(format!("transform-in({value})")).await?;
        Ok(format!("transform-out({result})"))
    }
);

streaming_middleware!(
    ShortCircuit,
    [name = "streaming-short-circuit"],
    |_underlying, value| { Ok(format!("short({value})")) }
);

streaming_middleware!(
    Repeated,
    [name = "streaming-repeated"],
    |underlying, value| {
        let first = underlying.apply(format!("repeat-a({value})")).await?;
        let second = underlying.apply(format!("repeat-b({value})")).await?;
        Ok(format!("repeated[{first}|{second}]"))
    }
);

streaming_middleware!(
    Overlapping,
    [name = "streaming-overlapping"],
    |underlying, value| {
        let left = underlying.apply(format!("overlap-left({value})"));
        let right = underlying.apply(format!("overlap-right({value})"));
        let (left, right) = (left, right).join().await;
        Ok(format!("overlapping[{}|{}]", left?, right?))
    }
);

streaming_middleware!(
    EarlyReturn,
    [name = "streaming-early-return"],
    |underlying, value| {
        let admitted = underlying
            .start_apply(format!("early-child({value})"))
            .await?;
        drop(admitted);
        Ok(format!("early-return({value})"))
    }
);

streaming_middleware!(
    RaceSettlement,
    [name = "streaming-race-settlement"],
    |underlying, value| {
        let loser = underlying
            .start_apply(format!("race-cancelled({value})"))
            .await?;
        wait_at_middleware_promise_checkpoint("middleware-race-cancel-ready").await;
        loser.cancel();
        let detached = underlying
            .start_apply(format!("race-detached({value})"))
            .await?;
        drop(detached);
        let sibling = underlying.apply(format!("race-sibling({value})")).await?;
        Ok(format!("race-winner[{sibling}]"))
    }
);

async fn wait_at_middleware_promise_checkpoint(name: &str) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::{wit_future, wit_stream};

    let promise = golem_rust::create_promise();
    let port = std::env::var("MIDDLEWARE_PROMISE_CHECKPOINT_PORT")
        .expect("middleware promise checkpoint port is configured");
    let headers = types::Fields::from_list(&[]).expect("valid checkpoint fields");
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
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
        .set_path_with_query(Some(&format!("/{name}")))
        .expect("set path");
    let payload = promise.oplog_idx.to_string().into_bytes();
    let send = async move { client::send(request).await.expect("send checkpoint") };
    let finish = async move {
        assert!(body_tx.write_all(payload).await.is_empty());
        drop(body_tx);
        trailers_tx.write(Ok(None)).await.expect("finish trailers");
        transmit.await.expect("transmit checkpoint");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
    golem_rust::await_promise(&promise).await;
}

fn append_lane_marker(marker: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/middleware-lanes.log")
        .expect("filesystem-capable middleware has the owner filesystem");
    file.write_all(marker.as_bytes())
        .expect("append middleware lane marker");
}

streaming_middleware!(
    FilesystemOuter,
    [name = "streaming-filesystem-outer"],
    |underlying, value| {
        append_lane_marker("O");
        underlying.apply(format!("fs-outer({value})")).await
    }
);

streaming_middleware!(
    IncapableFanout,
    [name = "streaming-incapable-fanout"],
    |underlying, value| {
        let left = underlying.apply(format!("fanout-left({value})"));
        let right = underlying.apply(format!("fanout-right({value})"));
        let (left, right) = (left, right).join().await;
        Ok(format!("fanout[{}|{}]", left?, right?))
    }
);

streaming_middleware!(
    FilesystemInner,
    [name = "streaming-filesystem-inner"],
    |underlying, value| {
        append_lane_marker(if value.starts_with("fanout-left(") {
            "L"
        } else {
            "R"
        });
        underlying.apply(format!("fs-inner({value})")).await
    }
);

streaming_middleware!(
    PartialFanout,
    [name = "streaming-partial-fanout"],
    |underlying, value| {
        let completed = underlying
            .start_apply(format!("partial-completed({value})"))
            .await?;
        let pending = underlying
            .start_apply(format!("partial-pending({value})"))
            .await?;
        let completed = completed.get().await?;
        let pending = pending.get().await?;
        Ok(format!("partial[{completed}|{pending}]"))
    }
);

#[derive(IntoSchema, FromSchema)]
struct PrefixParameters {
    prefix: String,
    rules: Vec<PrefixRule>,
}

#[derive(IntoSchema, FromSchema)]
struct PrefixRule {
    label: String,
    enabled: bool,
}

struct Parameterized {
    parameters: PrefixParameters,
}

impl Parameterized {
    fn new(parameters: PrefixParameters) -> Self {
        Self { parameters }
    }
}

#[tool_middleware(
    name = "streaming-parameterized",
    constructor = Parameterized::new,
    parameters = PrefixParameters
)]
impl MiddlewareProbeMiddleware for Parameterized {
    async fn apply(
        &self,
        underlying: &MiddlewareProbeUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<Infallible>> {
        let labels = self
            .parameters
            .rules
            .iter()
            .filter(|rule| rule.enabled)
            .map(|rule| rule.label.as_str())
            .collect::<Vec<_>>()
            .join(",");
        underlying
            .apply(format!("{}[{labels}]({value})", self.parameters.prefix))
            .await
    }
}
