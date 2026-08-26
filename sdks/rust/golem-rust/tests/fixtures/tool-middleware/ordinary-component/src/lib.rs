use golem_rust::{agent_definition, agent_implementation, tool_implementation};
use middleware_definition::{PublicEcho, PublicEchoClient, PublicError};

#[agent_definition]
trait OrdinaryFixtureAgent {
    fn new(name: String) -> Self;
    async fn ping(&self) -> String;
}

struct OrdinaryFixtureAgentImpl {
    name: String,
}

#[agent_implementation]
impl OrdinaryFixtureAgent for OrdinaryFixtureAgentImpl {
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

struct OrdinaryEcho;

#[tool_implementation]
impl PublicEcho for OrdinaryEcho {
    fn echo(&self, value: String) -> Result<String, PublicError> {
        Ok(value)
    }
}
