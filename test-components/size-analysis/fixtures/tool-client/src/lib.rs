use golem_rust::{ToolError, agent_definition, agent_implementation, tool_definition};

#[derive(ToolError)]
enum EchoError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Rejected(String),
}

#[tool_definition(version = "1.0.0")]
trait RemoteEcho {
    fn echo(&self, value: String) -> Result<String, EchoError>;
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
        match RemoteEchoClient::new().echo(value).await {
            Ok(value) => value,
            Err(golem_rust::agentic::ToolError::Tool(EchoError::Rejected(message))) => message,
            Err(golem_rust::agentic::ToolError::UnknownCustomError(error)) => error.name,
            Err(golem_rust::agentic::ToolError::Rpc(error)) => error.to_string(),
        }
    }
}
