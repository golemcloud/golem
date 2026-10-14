use middleware_definition::{PublicEchoMiddleware, PublicEchoUnderlying, PublicError};
use sdk::{tool::ToolInvokeError, tool_middleware};

struct Policy;

impl Policy {
    fn new() -> String {
        "not middleware state".to_string()
    }
}

#[tool_middleware(name = "invalid-return-constructor", constructor = Policy::new)]
impl PublicEchoMiddleware for Policy {
    async fn echo(
        &self,
        underlying: &PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}

fn main() {}
