///|
#derive.universal_tool_middleware("pure-audit")
pub async fn pure_audit(
  invocation : @toolMiddleware.RawToolInvocation,
  underlying : @toolMiddleware.UnderlyingTool,
) -> Result[
  @toolMiddleware.RawInvocationResult,
  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],
] {
  let ambient = @tool.ToolClient::new("ambient-audit")
  ambient.drop()
  underlying.invoke_raw(
    invocation.command_path(),
    invocation.input(),
    invocation.stdin(),
  )
}

///|
fn main {

}
