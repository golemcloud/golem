///|
pub(all) struct Handlers {
  discover_tool_middlewares : () -> Result[
    Array[@common.ToolMiddleware],
    @types.ToolError,
  ]
  get_tool_middleware : (String) -> Result[
    @common.ToolMiddleware,
    @types.ToolError,
  ]
  invoke_tool_middleware : async (
    String,
    String,
    @common.Tool,
    @types.TypedSchemaValue,
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Result[FixedArray[Byte], @streams.ByteStreamFailure]]?,
    @streams.ToolStdoutWriter?,
    @common0.Principal,
    @underlying.UnderlyingTool,
    @async-core.TaskGroup[Unit],
  ) -> Result[@common.InvocationResult, @types.ToolError]
}

///|
pub let handlers : Ref[Handlers?] = Ref(None)

///|
pub fn discover_tool_middlewares() -> Result[
  Array[@common.ToolMiddleware],
  @types.ToolError,
] {
  match handlers.val {
    Some(h) => (h.discover_tool_middlewares)()
    None => Ok([])
  }
}

///|
pub fn get_tool_middleware(
  name : String,
) -> Result[@common.ToolMiddleware, @types.ToolError] {
  match handlers.val {
    Some(h) => (h.get_tool_middleware)(name)
    None => Err(@types.ToolError::InvalidToolName(name))
  }
}

///|
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
  stdout : @streams.ToolStdoutWriter?,
  principal : @common0.Principal,
  wrapped : @underlying.UnderlyingTool,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[@common.InvocationResult, @types.ToolError] {
  match handlers.val {
    Some(h) =>
      (h.invoke_tool_middleware)(
        middleware_name, tool_name, tool_metadata, parameters, command_path, input,
        stdin, stdout, principal, wrapped, background_group,
      )
    None => {
      @model.drop_wit_capabilities(parameters.value)
      @model.drop_wit_capabilities(input.value)
      if stdin is Some(reader) {
        reader.drop_sync()
      }
      if stdout is Some(writer) {
        writer.drop()
      }
      wrapped.drop()
      Err(@types.ToolError::InvalidToolName(middleware_name))
    }
  }
}
