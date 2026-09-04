///|
/// Enumerate every middleware definition exported by this component.
pub fn discover_tool_middlewares() -> Result[
  Array[@common.ToolMiddleware],
  @common.ToolError,
] {
  Ok(@toolMiddleware.discover_tool_middlewares())
}

///|
/// Look up one middleware definition by canonical name.
pub fn get_tool_middleware(
  name : String,
) -> Result[@common.ToolMiddleware, @common.ToolError] {
  @toolMiddleware.get_registered_tool_middleware(name)
}

///|
/// Invoke one middleware layer around its runtime-bound next inner layer.
pub async fn invoke_tool_middleware(
  middleware_name : String,
  tool_name : String,
  tool_metadata : @common.Tool,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  principal : @common0.Principal,
  wrapped : @common.UnderlyingTool,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @common.ToolError] {
  let underlying = @toolMiddleware.UnderlyingTool::from_wit(
    wrapped, background_group,
  )
  @toolMiddleware.invoke_registered_tool_middleware(
    middleware_name, tool_name, tool_metadata, command_path, input, stdin, principal,
    underlying,
  )
}
