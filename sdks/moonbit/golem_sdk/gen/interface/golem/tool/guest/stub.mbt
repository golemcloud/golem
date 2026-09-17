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
  stdin : @async-core.Stream[
    Result[FixedArray[Byte], @streams.ByteStreamFailure],
  ]?,
  stdout : @streams.ToolStdoutWriter?,
  principal : @common0.Principal,
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @common.ToolError] {
  let stdin = stdin.map(@tool.byte_stream)
  match @tool.validate_tool_stdout(tool_name, command_path, stdout is Some(_)) {
    Err(error) => {
      if stdout is Some(writer) {
        writer.drop()
      }
      return @tool.reject_wire_invocation(input, stdin, error)
    }
    Ok(_) => ()
  }
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
      @tool.invoke_with_stdout(stdout, () => {
        @tool.invoke_registered_tool(
          tool_name,
          command_path,
          input,
          stdin,
          Some(stdout),
          principal,
        )
      })
    }
  }
}
