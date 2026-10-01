///|
/// Enumerate the tools this component exposes.
pub fn discover_tools() -> Result[Array[@common.Tool], @types.ToolError] {
  Ok(@tool.discover_tools())
}

///|
/// Look up a single tool by name.
pub fn get_tool(name : String) -> Result[@common.Tool, @types.ToolError] {
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
  stdout : @streams.ToolOutputWriter?,
  stderr : @streams.ToolOutputWriter?,
  principal : @common0.Principal,
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @types.ToolError] {
  let stdin = stdin.map(source => @tool.byte_stream(source))
  match
    @tool.validate_tool_outputs(
      tool_name,
      command_path,
      stdout is Some(_),
      stderr is Some(_),
    ) {
    Err(error) => {
      if stdout is Some(writer) {
        writer.drop()
      }
      if stderr is Some(writer) {
        writer.drop()
      }
      return @tool.reject_wire_invocation(input, stdin, error)
    }
    Ok(_) => ()
  }
  match (stdout, stderr) {
    (None, None) =>
      @tool.invoke_registered_tool(
        tool_name,
        command_path,
        input,
        stdin,
        None,
        None,
        principal,
      )
    (Some(writer), None) => {
      let stdout = @tool.provider_output(writer)
      @tool.invoke_with_output(stdout, () => {
        @tool.invoke_registered_tool(
          tool_name,
          command_path,
          input,
          stdin,
          Some(stdout),
          None,
          principal,
        )
      })
    }
    (None, Some(writer)) => {
      let stderr = @tool.provider_output(writer)
      @tool.invoke_with_output(stderr, () => {
        @tool.invoke_registered_tool(
          tool_name,
          command_path,
          input,
          stdin,
          None,
          Some(stderr),
          principal,
        )
      })
    }
    (Some(stdout_writer), Some(stderr_writer)) => {
      let stdout = @tool.provider_output(stdout_writer)
      let stderr = @tool.provider_output(stderr_writer)
      @tool.invoke_with_output(stdout, () => {
        @tool.invoke_with_output(stderr, () => {
          @tool.invoke_registered_tool(
            tool_name,
            command_path,
            input,
            stdin,
            Some(stdout),
            Some(stderr),
            principal,
          )
        })
      })
    }
  }
}
