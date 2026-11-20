///|
/// Enumerate every middleware definition exported by this component.
pub fn discover_tool_middlewares() -> Result[
  Array[@common.ToolMiddleware],
  @types.ToolError,
] {
  Ok(@toolMiddleware.discover_tool_middlewares())
}

///|
/// Look up one middleware definition by canonical name.
pub fn get_tool_middleware(
  name : String,
) -> Result[@common.ToolMiddleware, @types.ToolError] {
  @toolMiddleware.get_registered_tool_middleware(name)
}

///|
/// Invoke one middleware layer around its runtime-bound next inner layer.
pub async fn invoke_tool_middleware(
  middleware_name : String,
  tool_name : String,
  tool_metadata : @common.Tool,
  parameters : @types.TypedSchemaValue,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[
    Result[FixedArray[Byte], @streams.ByteStreamFailure],
  ]?,
  stdout : @streams.ToolOutputWriter?,
  stderr : @streams.ToolOutputWriter?,
  wrapped : @underlying.UnderlyingTool,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @types.ToolError] {
  let underlying = @toolMiddleware.UnderlyingTool::from_wit(
    wrapped, background_group,
  )
  let context = @caller.InvocationContext::current()
  defer context.end()
  let result = @toolMiddleware.invoke_registered_tool_middleware_wire(
    middleware_name, tool_name, tool_metadata, parameters, command_path, input, stdin,
    stdout, stderr, context, underlying,
  )
  // A result that holds a stream keeps the caller for its producer.
  context.end(
    returned_stream=result is Ok(invocation) &&
      (invocation.stdout is Some(_) ||
      invocation.stderr is Some(_) ||
      (invocation.result is Some(typed) && @caller.holds_stream(typed.value))),
  )
  result
}
