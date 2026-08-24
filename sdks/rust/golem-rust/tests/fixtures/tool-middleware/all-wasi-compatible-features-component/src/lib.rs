use golem_rust::{
    agent_definition, agent_implementation, tool_implementation, tool_middleware,
    tool::ToolInvokeError,
};
use middleware_definition::{
    PublicEcho, PublicEchoClient, PublicEchoMiddleware, PublicEchoUnderlying, PublicError,
};

#[agent_definition]
trait AllExportsFixtureAgent {
    fn new(name: String) -> Self;
    async fn ping(&self) -> String;
}

struct AllExportsFixtureAgentImpl {
    name: String,
}

#[agent_implementation]
impl AllExportsFixtureAgent for AllExportsFixtureAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    async fn ping(&self) -> String {
        match PublicEchoClient::default().echo(self.name.clone()).await {
            Ok(value) => value,
            Err(_) => "ambient tool invocation failed".to_string(),
        }
    }
}

struct AllExportsEcho;

#[tool_implementation]
impl PublicEcho for AllExportsEcho {
    fn echo(&self, value: String) -> Result<String, PublicError> {
        Ok(value)
    }
}

struct AllExportsPolicy;

impl AllExportsPolicy {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "fixture-all-exports", constructor = AllExportsPolicy::new)]
impl PublicEchoMiddleware for AllExportsPolicy {
    async fn echo(
        &self,
        underlying: &mut PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}
