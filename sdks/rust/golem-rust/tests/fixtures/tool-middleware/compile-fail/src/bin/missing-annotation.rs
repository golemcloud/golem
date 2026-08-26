use middleware_definition::{PublicEchoMiddleware, PublicEchoUnderlying, PublicError};
use sdk::tool::ToolInvokeError;

struct Policy;

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
