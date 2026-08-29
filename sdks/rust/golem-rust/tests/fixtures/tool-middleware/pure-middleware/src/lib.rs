use middleware_definition::{
    BackendEchoUnderlying, BackendError, PublicEchoMiddleware, PublicEchoUnderlying, PublicError,
};
use sdk::{tool_middleware, tool::ToolInvokeError};

struct Transparent;

impl Transparent {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "fixture-transparent", constructor = Transparent::new)]
impl PublicEchoMiddleware for Transparent {
    async fn echo(
        &self,
        underlying: &mut PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}

struct Adapter;

impl Adapter {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "fixture-adapter", constructor = Adapter::new)]
impl PublicEchoMiddleware<BackendEchoUnderlying> for Adapter {
    async fn echo(
        &self,
        underlying: &mut BackendEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying
            .execute(format!("backend:{value}"))
            .await
            .map(|value| value.to_string())
            .map_err(|error| {
                error.map_tool(|BackendError::Failed(message)| PublicError::Rejected(message))
            })
    }
}
