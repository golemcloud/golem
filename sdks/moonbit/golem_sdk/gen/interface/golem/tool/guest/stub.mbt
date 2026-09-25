///|
pub(all) struct Handlers {
  discover_tools : () -> Result[Array[@common.Tool], @types.ToolError]
  get_tool : (String) -> Result[@common.Tool, @types.ToolError]
  invoke : async (
    String,
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Result[FixedArray[Byte], @streams.ByteStreamFailure]]?,
    @streams.ToolStdoutWriter?,
    @common0.Principal,
    @async-core.TaskGroup[Unit],
  ) -> Result[@common.InvocationResult, @types.ToolError]
}

///|
pub let handlers : Ref[Handlers?] = Ref(None)

///|
pub fn discover_tools() -> Result[Array[@common.Tool], @types.ToolError] {
  match handlers.val {
    Some(h) => (h.discover_tools)()
    None => Ok([])
  }
}

///|
pub fn get_tool(name : String) -> Result[@common.Tool, @types.ToolError] {
  match handlers.val {
    Some(h) => (h.get_tool)(name)
    None => Err(@types.ToolError::InvalidToolName(name))
  }
}

///|
pub async fn invoke(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[
    Result[FixedArray[Byte], @streams.ByteStreamFailure],
  ]?,
  stdout : @streams.ToolStdoutWriter?,
  principal : @common0.Principal,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @types.ToolError] {
  match handlers.val {
    Some(h) =>
      (h.invoke)(
        tool_name, command_path, input, stdin, stdout, principal, background_group,
      )
    None => {
      @model.drop_wit_capabilities(input.value)
      if stdin is Some(reader) {
        reader.drop_sync()
      }
      if stdout is Some(writer) {
        writer.drop()
      }
      Err(@types.ToolError::InvalidToolName(tool_name))
    }
  }
}
