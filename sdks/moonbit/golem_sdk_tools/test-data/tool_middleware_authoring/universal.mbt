/// Audits any runtime-bound tool without rebuilding its metadata or schema carriers.
#derive.universal_tool_middleware("audit", alias="trace-tools")
pub async fn audit(
  invocation : @toolMiddleware.RawToolInvocation,
  underlying : @toolMiddleware.UnderlyingTool,
) -> Result[
  @toolMiddleware.RawInvocationResult,
  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],
] {
  ignore(invocation.tool_name())
  ignore(invocation.tool_metadata())
  ignore(invocation.principal())
  underlying.invoke_raw(
    invocation.command_path(),
    invocation.input(),
    invocation.stdin(),
  )
}
