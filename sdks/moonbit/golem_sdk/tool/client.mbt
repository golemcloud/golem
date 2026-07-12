///|
pub(all) enum NoToolError {}

///|
pub(all) enum RpcError {
  Protocol(String)
  Denied(String)
  NotFound(String)
  RemoteInternal(String)
} derive(Eq, Debug)

///|
pub(all) enum ToolError[E] {
  Rpc(RpcError)
  Tool(E)
}

///|
pub(all) struct InvocationResult {
  result : @model.TypedSchemaValue?
  stdout : @streams.OutputStream?
}

///|
pub(all) struct ToolClient {
  rpc : @toolHost.ToolRpc
}

///|
pub fn ToolClient::new(tool_name : String) -> ToolClient {
  { rpc: @toolHost.ToolRpc::tool_rpc(tool_name) }
}

///|
pub fn ToolClient::drop(self : ToolClient) -> Unit {
  self.rpc.drop()
}

///|
pub fn[E] ToolClient::invoke_and_await(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @streams.InputStream?,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await(self.rpc, command_path, input, stdin, decode_error)
}

///|
pub fn[E : @schema.FromSchema] ToolClient::invoke_and_await_payload_error(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @streams.InputStream?,
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await_payload_error(self.rpc, command_path, input, stdin)
}

///|
pub fn ToolClient::invoke_and_await_no_tool_error(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @streams.InputStream?,
) -> Result[InvocationResult, ToolError[NoToolError]] {
  invoke_and_await_no_tool_error(self.rpc, command_path, input, stdin)
}

///|
pub fn[E] invoke_and_await(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @streams.InputStream?,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> Result[InvocationResult, ToolError[E]] {
  let wire_input = @model.typed_schema_value_to_wit(input) catch {
    e => return Err(tool_protocol_error("failed to encode tool input: \{e}"))
  }
  let wire_result = match
    rpc.invoke_and_await(command_path, wire_input, stdin) {
    Ok(result) => result
    Err(e) => return Err(map_rpc_error(e, decode_error))
  }
  decode_invocation_result(wire_result)
}

///|
pub fn[E : @schema.FromSchema] invoke_and_await_payload_error(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @streams.InputStream?,
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await(rpc, command_path, input, stdin, decode_custom_tool_error)
}

///|
pub fn invoke_and_await_no_tool_error(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @streams.InputStream?,
) -> Result[InvocationResult, ToolError[NoToolError]] {
  invoke_and_await(rpc, command_path, input, stdin, fn(_value) {
    Err("remote custom tool error is not declared by this client")
  })
}

///|
fn[E] decode_invocation_result(
  result : @toolCommon.InvocationResult,
) -> Result[InvocationResult, ToolError[E]] {
  let decoded = match result.result {
    Some(value) => {
      let decoded_value = @model.typed_schema_value_from_wit(value) catch {
        e =>
          return Err(tool_protocol_error("failed to decode tool result: \{e}"))
      }
      Some(decoded_value)
    }
    None => None
  }
  Ok({ result: decoded, stdout: result.stdout })
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_with_stdout(
  result : InvocationResult,
) -> Result[(T, @streams.OutputStream), ToolError[E]] {
  let stdout = expect_stdout(result.stdout)
  let value = decode_expected_value(result.result)
  match (stdout, value) {
    (Ok(stdout), Ok(value)) => Ok((value, stdout))
    (Err(e), _) => Err(e)
    (_, Err(e)) => Err(e)
  }
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_value(
  result : InvocationResult,
) -> Result[T, ToolError[E]] {
  match expect_no_stdout(result.stdout) {
    Err(e) => Err(e)
    Ok(_) => decode_expected_value(result.result)
  }
}

///|
pub fn[E] decode_result_stdout_only(
  result : InvocationResult,
) -> Result[@streams.OutputStream, ToolError[E]] {
  let stdout = expect_stdout(result.stdout)
  match (stdout, expect_no_value(result.result)) {
    (Ok(stdout), Ok(_)) => Ok(stdout)
    (Err(e), _) => Err(e)
    (_, Err(e)) => Err(e)
  }
}

///|
pub fn[E] decode_result_empty(
  result : InvocationResult,
) -> Result[Unit, ToolError[E]] {
  match expect_no_stdout(result.stdout) {
    Err(e) => Err(e)
    Ok(_) => expect_no_value(result.result)
  }
}

///|
pub fn[E] expect_stdout(
  stdout : @streams.OutputStream?,
) -> Result[@streams.OutputStream, ToolError[E]] {
  match stdout {
    Some(stdout) => Ok(stdout)
    None =>
      Err(
        tool_protocol_error(
          "tool result did not contain declared stdout stream",
        ),
      )
  }
}

///|
pub fn[E] expect_no_stdout(
  stdout : @streams.OutputStream?,
) -> Result[Unit, ToolError[E]] {
  match stdout {
    Some(_) =>
      Err(
        tool_protocol_error("tool result unexpectedly contained stdout stream"),
      )
    None => Ok(())
  }
}

///|
pub fn[E] expect_value(
  value : @model.TypedSchemaValue?,
) -> Result[@model.TypedSchemaValue, ToolError[E]] {
  match value {
    Some(value) => Ok(value)
    None => Err(tool_protocol_error("tool result did not contain a value"))
  }
}

///|
pub fn[E] expect_no_value(
  value : @model.TypedSchemaValue?,
) -> Result[Unit, ToolError[E]] {
  match value {
    Some(_) =>
      Err(tool_protocol_error("tool result unexpectedly contained a value"))
    None => Ok(())
  }
}

///|
pub fn[E] tool_protocol_error(message : String) -> ToolError[E] {
  ToolError::Rpc(RpcError::Protocol(message))
}

///|
fn[T : @schema.FromSchema, E] decode_expected_value(
  value : @model.TypedSchemaValue?,
) -> Result[T, ToolError[E]] {
  let value = match expect_value(value) {
    Ok(value) => value
    Err(e) => return Err(e)
  }
  try @schema.from_value_as(value.value) catch {
    e => Err(tool_protocol_error("failed to decode tool result: \{e}"))
  } noraise {
    decoded => Ok(decoded)
  }
}

///|
fn[E : @schema.FromSchema] decode_custom_tool_error(
  value : @model.TypedSchemaValue,
) -> Result[E, String] {
  try @schema.from_value_as(value.value) catch {
    e => Err("failed to decode remote tool error: \{e}")
  } noraise {
    decoded => Ok(decoded)
  }
}

///|
fn[E] map_rpc_error(
  error : @toolHost.RpcError,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> ToolError[E] {
  match error {
    @toolHost.RpcError::ProtocolError(message) =>
      ToolError::Rpc(RpcError::Protocol(message))
    @toolHost.RpcError::Denied(message) =>
      ToolError::Rpc(RpcError::Denied(message))
    @toolHost.RpcError::NotFound(message) =>
      ToolError::Rpc(RpcError::NotFound(message))
    @toolHost.RpcError::RemoteInternalError(message) =>
      ToolError::Rpc(RpcError::RemoteInternal(message))
    @toolHost.RpcError::RemoteToolError(error) =>
      map_remote_tool_error(error, decode_error)
  }
}

///|
fn[E] map_remote_tool_error(
  error : @toolCommon.ToolError,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> ToolError[E] {
  match error {
    @toolCommon.ToolError::CustomError(value) => {
      let decoded = @model.typed_schema_value_from_wit(value) catch {
        e =>
          return tool_protocol_error("failed to decode remote tool error: \{e}")
      }
      match decode_error(decoded) {
        Ok(error) => ToolError::Tool(error)
        Err(message) => ToolError::Rpc(RpcError::Protocol(message))
      }
    }
    other =>
      ToolError::Rpc(
        RpcError::Protocol(
          "remote tool error: " + remote_tool_error_label(other),
        ),
      )
  }
}

///|
fn remote_tool_error_label(error : @toolCommon.ToolError) -> String {
  match error {
    @toolCommon.ToolError::InvalidToolName(name) =>
      "invalid tool name `" + name + "`"
    @toolCommon.ToolError::InvalidCommandPath(path) =>
      "invalid command path `" + path.join(" ") + "`"
    @toolCommon.ToolError::InvalidInput(message) => "invalid input: " + message
    @toolCommon.ToolError::ConstraintViolation(message) =>
      "constraint violation: " + message
    @toolCommon.ToolError::InvalidResult(message) =>
      "invalid result: " + message
    @toolCommon.ToolError::CustomError(_) => "custom error"
  }
}
