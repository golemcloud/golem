use golem_rust::{
    agent_definition, agent_implementation, tool_implementation, tool_middleware,
    tool::ToolInvokeError,
};
use middleware_definition::{
    PublicEcho, PublicEchoClient, PublicEchoMiddleware, PublicEchoUnderlying, PublicError,
};

#[agent_definition]
trait CombinedFixtureAgent {
    fn new(name: String) -> Self;
    async fn ping(&self) -> String;
}

struct CombinedFixtureAgentImpl {
    name: String,
}

#[agent_implementation]
impl CombinedFixtureAgent for CombinedFixtureAgentImpl {
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

struct CombinedEcho;

#[tool_implementation]
impl PublicEcho for CombinedEcho {
    fn echo(&self, value: String) -> Result<String, PublicError> {
        Ok(value)
    }
}

struct CombinedPolicy;

impl CombinedPolicy {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(name = "fixture-combined", constructor = CombinedPolicy::new)]
impl PublicEchoMiddleware for CombinedPolicy {
    async fn echo(
        &self,
        underlying: &mut PublicEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<PublicError>> {
        underlying.echo(value).await
    }
}
