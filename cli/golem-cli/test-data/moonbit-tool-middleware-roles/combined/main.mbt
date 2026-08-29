///|
#derive.agent
struct CombinedAgent {
  name : String
}

///|
fn CombinedAgent::new(name : String) -> CombinedAgent {
  { name, }
}

///|
pub fn CombinedAgent::get_name(self : Self) -> String {
  self.name
}

///|
pub fn CombinedAgent::use_ambient_tool(self : Self) -> Unit {
  let client = @tool.ToolClient::new(self.name)
  client.drop()
}

///|
#derive.universal_tool_middleware("combined-audit")
pub async fn combined_audit(
  invocation : @toolMiddleware.RawToolInvocation,
  underlying : @toolMiddleware.UnderlyingTool,
) -> Result[
  @toolMiddleware.RawInvocationResult,
  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],
] {
  underlying.invoke_raw(
    invocation.command_path(),
    invocation.input(),
    invocation.stdin(),
  )
}

///|
fn main {

}
