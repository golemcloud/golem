use futures_concurrency::prelude::*;
use golem_rust::tool::{
    InputStream, InvocationResult, Principal, RawCustomToolError, Tool, ToolInvokeError,
    UnderlyingTool,
};
use golem_rust::{TypedSchemaValue, tool_definition, tool_middleware, universal_tool_middleware};
use std::convert::Infallible;
use std::future::Future;
use std::task::Poll;

#[tool_definition(version = "1.0.0")]
pub trait MiddlewareProbe {
    async fn apply(&self, value: String) -> String;
}

#[universal_tool_middleware(name = "streaming-universal-pass-through")]
async fn universal_pass_through(
    _tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    underlying.invoke(command_path, input, stdin).await
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
                $underlying: &mut MiddlewareProbeUnderlying,
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
        let mut call = std::pin::pin!(underlying.apply(format!("early-child({value})")));
        std::future::poll_fn(|context| {
            let _ = call.as_mut().poll(context);
            Poll::Ready(())
        })
        .await;
        Ok(format!("early-return({value})"))
    }
);
