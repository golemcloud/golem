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
pub async fn invoke(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Result[FixedArray[Byte], @host.ByteStreamFailure]]?,
  stdout : @host.ToolStdoutWriter?,
  principal : @common0.Principal,
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @common.ToolError] {
  let stdin = stdin.map(@tool.byte_stream)
  match stdout {
    None =>
      @tool.invoke_registered_tool(
        tool_name,
        command_path,
        input,
        stdin,
        None,
        principal,
      )
    Some(writer) => {
      let stdout = @tool.provider_stdout(writer)
      defer stdout.drop()
      let result = @tool.invoke_registered_tool(
        tool_name,
        command_path,
        input,
        stdin,
        Some(stdout.output()),
        principal,
      )
      stdout.finish()
      result
    }
  }
}
