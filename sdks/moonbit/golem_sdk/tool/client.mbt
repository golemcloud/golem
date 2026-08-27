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
pub async fn[E] ToolClient::invoke_and_await(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await(self.rpc, command_path, input, stdin, decode_error)
}

///|
pub async fn[E : @schema.FromSchema] ToolClient::invoke_and_await_payload_error(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await_payload_error(self.rpc, command_path, input, stdin)
}

///|
pub async fn[E : ToolErrorSchema] ToolClient::invoke_and_await_tool_error(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await_tool_error(self.rpc, command_path, input, stdin)
}

///|
pub async fn ToolClient::invoke_and_await_no_tool_error(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
) -> Result[InvocationResult, ToolError[NoToolError]] {
  invoke_and_await_no_tool_error(self.rpc, command_path, input, stdin)
}

///|
pub async fn[E] invoke_and_await(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> Result[InvocationResult, ToolError[E]] {
  let wire_input = match @toolCore.encode_typed_input(input) {
    Ok(encoded) => encoded
    Err(error) => return Err(tool_protocol_error(error))
  }
  let wire_result = match
    rpc.invoke_and_await(command_path, wire_input, stdin) {
    Ok(result) => result
    Err(e) => return Err(map_rpc_error(e, decode_error))
  }
  match @toolCore.decode_invocation_result(wire_result) {
    Ok(result) => Ok(result)
    Err(error) => Err(tool_protocol_error(error))
  }
}

///|
pub async fn[E : @schema.FromSchema] invoke_and_await_payload_error(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await(rpc, command_path, input, stdin, decode_custom_tool_error)
}

///|
pub async fn[E : ToolErrorSchema] invoke_and_await_tool_error(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
) -> Result[InvocationResult, ToolError[E]] {
  invoke_and_await(rpc, command_path, input, stdin, fn(value) {
    E::from_error_payload_value(value)
  })
}

///|
pub async fn invoke_and_await_no_tool_error(
  rpc : @toolHost.ToolRpc,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
) -> Result[InvocationResult, ToolError[NoToolError]] {
  invoke_and_await(rpc, command_path, input, stdin, fn(_value) {
    Err("remote custom tool error is not declared by this client")
  })
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_with_stdout(
  result : InvocationResult,
) -> Result[(T, @async-core.Stream[Byte]), ToolError[E]] {
  match @toolCore.project_result_with_stdout(result) {
    Ok(projected) => Ok(projected)
    Err(error) => Err(tool_protocol_error(error))
  }
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_value(
  result : InvocationResult,
) -> Result[T, ToolError[E]] {
  match @toolCore.project_result_value(result) {
    Ok(value) => Ok(value)
    Err(error) => Err(tool_protocol_error(error))
  }
}

///|
pub fn[E] decode_result_stdout_only(
  result : InvocationResult,
) -> Result[@async-core.Stream[Byte], ToolError[E]] {
  match @toolCore.project_result_stdout_only(result) {
    Ok(stdout) => Ok(stdout)
    Err(error) => Err(tool_protocol_error(error))
  }
}

///|
pub fn[E] decode_result_empty(
  result : InvocationResult,
) -> Result[Unit, ToolError[E]] {
  match @toolCore.project_result_empty(result) {
    Ok(_) => Ok(())
    Err(error) => Err(tool_protocol_error(error))
  }
}

///|
pub fn[E] expect_stdout(
  stdout : @async-core.Stream[Byte]?,
) -> Result[@async-core.Stream[Byte], ToolError[E]] {
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
  stdout : @async-core.Stream[Byte]?,
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
fn[E : @schema.FromSchema] decode_custom_tool_error(
  value : @model.TypedSchemaValue,
) -> Result[E, String] {
  @toolCore.decode_custom_error(value)
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
      let decoded = match
        @toolCore.decode_typed_value(value, "remote tool error") {
        Ok(decoded) => decoded
        Err(message) => return tool_protocol_error(message)
      }
      match decode_error(decoded) {
        Ok(error) => ToolError::Tool(error)
        Err(message) => {
          @model.drop_owned_capabilities(decoded.value)
          ToolError::Rpc(RpcError::Protocol(message))
        }
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
