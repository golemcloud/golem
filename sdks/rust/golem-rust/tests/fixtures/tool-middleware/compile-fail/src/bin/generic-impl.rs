use middleware_definition::{PublicEchoMiddleware, PublicEchoUnderlying, PublicError};
use sdk::{tool::ToolInvokeError, tool_middleware};
use std::marker::PhantomData;

struct Policy<T>(PhantomData<T>);

impl<T> Policy<T> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

#[tool_middleware(name = "invalid-generic-impl", constructor = Policy::<T>::new)]
impl<T> PublicEchoMiddleware for Policy<T> {
    async fn echo(
        &self,
        underlying: &PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}

fn main() {}
