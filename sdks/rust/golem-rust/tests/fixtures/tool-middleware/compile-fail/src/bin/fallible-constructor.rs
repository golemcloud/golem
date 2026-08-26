use middleware_definition::{PublicEchoMiddleware, PublicEchoUnderlying, PublicError};
use sdk::{tool_middleware, tool::ToolInvokeError};

struct Policy;

impl Policy {
    fn new() -> Result<Self, std::convert::Infallible> {
        Ok(Self)
    }
}

#[tool_middleware(name = "invalid-fallible-constructor", constructor = Policy::new)]
impl PublicEchoMiddleware for Policy {
    async fn echo(
        &self,
        underlying: &mut PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}

fn main() {}
