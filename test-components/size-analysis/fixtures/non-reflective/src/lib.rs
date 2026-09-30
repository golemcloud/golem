use golem_rust::{
    agent_definition, agent_implementation, tool_definition, tool_implementation,
};

#[tool_definition(version = "1.0.0")]
trait EchoTool {
    fn echo(&self, value: String) -> String;
}

struct EchoToolImpl;

#[tool_implementation]
impl EchoTool for EchoToolImpl {
    fn echo(&self, value: String) -> String {
        value
    }
}

#[agent_definition]
trait Caller {
    fn new() -> Self;
    async fn call(&self, value: String) -> String;
}

struct CallerImpl;

#[agent_implementation]
impl Caller for CallerImpl {
    fn new() -> Self {
        Self
    }

    async fn call(&self, value: String) -> String {
        EchoClient::get().echo(value).await
    }
}

#[agent_definition]
trait Echo {
    fn new() -> Self;
    fn echo(&self, value: String) -> String;
}

struct EchoImpl;

#[agent_implementation]
impl Echo for EchoImpl {
    fn new() -> Self {
        Self
    }

    fn echo(&self, value: String) -> String {
        value
    }
}
