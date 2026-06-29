///|
/// Enumerate the tools this component exposes.
pub fn discover_tools() -> Result[Array[@common.Tool], @common.ToolError] {
  ignore(@host.get_all_tools())
  Ok([])
}

///|
/// Look up a single tool by name.
pub fn get_tool(name : String) -> Result[@common.Tool, @common.ToolError] {
  Err(@common.ToolError::InvalidToolName(name))
}

///|
/// Invoke a command of a tool.
pub fn invoke(
  tool_name : String,
  _command_path : Array[String],
  _input : @types.TypedSchemaValue,
  _stdin : @streams.InputStream?,
  _principal : @common0.Principal,
) -> Result[@common.InvocationResult, @common.ToolError] {
  Err(@common.ToolError::InvalidToolName(tool_name))
}
