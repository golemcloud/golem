///|
pub async fn[E] ToolClient::invoke_and_await_wire(
  self : ToolClient,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  decode_error : (String, @types.TypedSchemaValue) -> Result[E, String]?,
) -> Result[@toolCommon.InvocationResult, ToolError[E]] {
  match
    self.rpc.invoke_and_await(command_path, input, prepare_stdin(stdin), None) {
    Ok(result) => Ok(result)
    Err(error) => Err(map_wire_rpc_error(error, decode_error))
  }
}

///|
/// A decoder returning Some owns the error payload, including failure cleanup.
/// None leaves ownership with the returned unknown error.
fn[E] map_wire_rpc_error(
  error : @types.ToolRpcError,
  decode_error : (String, @types.TypedSchemaValue) -> Result[E, String]?,
) -> ToolError[E] {
  match error {
    @types.ProtocolError(message) => Rpc(Protocol(message))
    @types.Denied(message) => Rpc(Denied(message))
    @types.NotFound(message) => Rpc(NotFound(message))
    @types.RemoteInternalError(message) => Rpc(RemoteInternal(message))
    @types.Cancelled => Rpc(Cancelled)
    @types.ResourceExhausted(message) => Rpc(ResourceExhausted(message))
    @types.RemoteToolError(@types.CustomError(error)) =>
      match decode_error(error.name, error.payload) {
        Some(Ok(value)) => Tool(value)
        Some(Err(message)) => tool_protocol_error(message)
        None => UnknownToolError(error.name, error.payload)
      }
    @types.RemoteToolError(other) =>
      tool_protocol_error(
        "remote tool error: " + remote_tool_error_label(other),
      )
  }
}

///|
pub fn[T : @schema.FromWire, E] decode_wire_result_value(
  result : @toolCommon.InvocationResult,
) -> Result[T, ToolError[E]] {
  if expect_no_stdout(result.stdout) is Err(error) {
    if result.result is Some(value) {
      @schema.WireReader::new(value.value).abort()
    }
    return Err(error)
  }
  guard result.result is Some(value) else {
    return Err(tool_protocol_error("tool result did not contain a value"))
  }
  try @schema.from_wire(value.value) catch {
    error => Err(tool_protocol_error(repr(error)))
  } noraise {
    value => Ok(value)
  }
}

///|
pub fn[E] decode_wire_result_empty(
  result : @toolCommon.InvocationResult,
) -> Result[Unit, ToolError[E]] {
  let stdout = expect_no_stdout(result.stdout)
  if result.result is Some(value) {
    @schema.WireReader::new(value.value).abort()
    return Err(
      tool_protocol_error("tool result unexpectedly contained a value"),
    )
  }
  stdout
}

///|
/// Applies a text annotation to a freshly encoded concrete input field.
pub fn wire_text_input_field(
  tree : @types.SchemaValueTree,
  field : Int,
) -> Unit {
  guard tree.value_nodes[tree.root] is @types.RecordValue(fields) else {
    abort("generated input must be a record")
  }
  fn refine(index : Int) -> Unit {
    match tree.value_nodes[index] {
      @types.StringValue(text) =>
        tree.value_nodes[index] = @types.TextValue({ text, language: None, })
      @types.OptionValue(Some(child)) => refine(child)
      @types.OptionValue(None) => ()
      @types.ListValue(children) =>
        for child in children {
          refine(child)
        }
      _ => abort("text annotation requires a string input")
    }
  }
  refine(fields[field])
}
