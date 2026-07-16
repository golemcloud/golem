///|
/// Enumerate the tools this component exposes.
pub fn discover_tools() -> Result[Array[@common.Tool], @common.ToolError] {
  Ok(@tool.discover_tools())
}

///|
/// Look up a single tool by name.
pub fn get_tool(name : String) -> Result[@common.Tool, @common.ToolError] {
  @tool.get_registered_tool(name)
}

///|
/// Invoke a command of a tool.
pub fn invoke(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @streams.InputStream?,
  principal : @common0.Principal,
) -> Result[@common.InvocationResult, @common.ToolError] {
  @tool.invoke_registered_tool(tool_name, command_path, input, stdin, principal)
}
