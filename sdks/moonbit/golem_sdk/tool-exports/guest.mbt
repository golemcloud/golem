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
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @types.ToolError] {
  let context = @tool.InvocationContext::current()
  defer context.end()
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
  let result = match (stdout, stderr) {
    (None, None) =>
      @tool.invoke_registered_tool(
        tool_name,
        command_path,
        input,
        stdin,
        None,
        None,
        context,
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
          context,
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
          context,
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
            context,
          )
        })
      })
    }
  }
  // A result that holds a stream keeps the caller for its producer.
  context.end(
    returned_stream=result is Ok(invocation) &&
      (
        invocation.stdout is Some(_) ||
        invocation.stderr is Some(_) ||
        (invocation.result is Some(typed) && @caller.holds_stream(typed.value))
      ),
  )
  result
}
