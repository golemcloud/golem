/// An agent exported by a combined component.
#derive.agent
struct CombinedAgent {}

fn CombinedAgent::new() -> CombinedAgent {
  {}
}

pub fn CombinedAgent::status(self : Self) -> String {
  ignore(self)
  "ready"
}

/// An ordinary tool exported by the same component.
#derive.tool("echo")
struct EchoTool {}

pub fn EchoTool::echo(value : String) -> String {
  value
}

/// Middleware exported together with the agent and ordinary tool.
#derive.tool_middleware("echo-policy", presented="EchoTool")
struct EchoPolicy {}

pub async fn EchoPolicy::echo(
  underlying : EchoToolUnderlying,
  value : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.echo(value)
}
