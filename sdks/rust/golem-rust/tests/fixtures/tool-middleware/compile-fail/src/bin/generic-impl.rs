use middleware_definition::{PublicEchoMiddleware, PublicEchoUnderlying, PublicError};
use sdk::{tool_middleware, tool::ToolInvokeError};
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
        underlying: &mut PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}

fn main() {}
