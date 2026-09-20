use futures_concurrency::prelude::*;
use golem_rust::agentic::{AgentStream, spawn_local};
use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
    ToolInvokeError, UnderlyingTool,
};
use golem_rust::{
    FromSchema, IntoSchema, TypedSchemaValue, tool_definition, tool_middleware,
    universal_tool_middleware,
};
use std::convert::Infallible;

#[tool_definition(version = "1.0.0")]
pub trait MiddlewareProbe {
    async fn apply(&self, value: String) -> String;
}

mod expected_typed_output {
    use super::*;

    #[derive(IntoSchema, FromSchema)]
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

    #[derive(IntoSchema, FromSchema)]
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

    #[derive(IntoSchema, FromSchema)]
    pub struct Item {
        pub ordinal: u32,
        pub caller_extra: u64,
        pub label: String,
    }

    #[derive(IntoSchema, FromSchema)]
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
