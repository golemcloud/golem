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
fn release_optional_value(
  value : @model.TypedSchemaValue?,
  drops : @model.SchemaValueResourceDrops,
) -> Unit {
  match value {
    Some(value) => value.value.release_resources(drops)
    None => ()
  }
}

///|
pub(all) struct ToolClient {
  rpc : @toolHost.ToolRpc
}

///|
pub fn ToolClient::new(tool_name : String) -> ToolClient {
  { rpc: @toolHost.ToolRpc::tool_rpc(tool_name), }
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
  let wire_input = @model.typed_schema_value_to_wit_async(input) catch {
    e => {
      release_failed_tool_input(input, @model_host.resource_drops())
      match stdin {
        Some(stdin) => stdin.drop_sync()
        None => ()
      }
      return Err(tool_protocol_error("failed to encode tool input: \{Repr(e)}"))
    }
  }
  let wire_result = match
    rpc.invoke_and_await(command_path, wire_input, stdin) {
    Ok(result) => result
    Err(e) => return Err(map_rpc_error(e, decode_error))
  }
  decode_invocation_result(wire_result)
}

///|
fn release_failed_tool_input(
  input : @model.TypedSchemaValue,
  drops : @model.SchemaValueResourceDrops,
) -> Unit {
  input.value.release_resources(drops)
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
fn[E] decode_invocation_result(
  result : @toolCommon.InvocationResult,
) -> Result[InvocationResult, ToolError[E]] {
  let decoded = match result.result {
    Some(value) => {
      let decoded_value = @model_host.typed_schema_value_from_wit(value) catch {
        e => {
          match result.stdout {
            Some(stdout) => stdout.drop_sync()
            None => ()
          }
          return Err(
            tool_protocol_error("failed to decode tool result: \{Repr(e)}"),
          )
        }
      }
      Some(decoded_value)
    }
    None => None
  }
  Ok({ result: decoded, stdout: result.stdout, })
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_with_stdout(
  result : InvocationResult,
) -> Result[(T, @async-core.Stream[Byte]), ToolError[E]] {
  decode_result_with_stdout_with_resource_drops(
    result,
    @model_host.resource_drops(),
  )
}

///|
fn[T : @schema.FromSchema, E] decode_result_with_stdout_with_resource_drops(
  result : InvocationResult,
  drops : @model.SchemaValueResourceDrops,
) -> Result[(T, @async-core.Stream[Byte]), ToolError[E]] {
  let stdout = match expect_stdout(result.stdout) {
    Ok(stdout) => stdout
    Err(e) => {
      release_optional_value(result.result, drops)
      return Err(e)
    }
  }
  match decode_expected_value(result.result, drops) {
    Ok(value) => Ok((value, stdout))
    Err(e) => {
      stdout.drop_sync()
      Err(e)
    }
  }
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_value(
  result : InvocationResult,
) -> Result[T, ToolError[E]] {
  decode_result_value_with_resource_drops(result, @model_host.resource_drops())
}

///|
fn[T : @schema.FromSchema, E] decode_result_value_with_resource_drops(
  result : InvocationResult,
  drops : @model.SchemaValueResourceDrops,
) -> Result[T, ToolError[E]] {
  match expect_no_stdout(result.stdout) {
    Err(e) => {
      release_optional_value(result.result, drops)
      Err(e)
    }
    Ok(_) => decode_expected_value(result.result, drops)
  }
}

///|
pub fn[E] decode_result_stdout_only(
  result : InvocationResult,
) -> Result[@async-core.Stream[Byte], ToolError[E]] {
  let drops = @model_host.resource_drops()
  let stdout = match expect_stdout(result.stdout) {
    Ok(stdout) => stdout
    Err(e) => {
      release_optional_value(result.result, drops)
      return Err(e)
    }
  }
  match expect_no_value_with_resource_drops(result.result, drops) {
    Ok(_) => Ok(stdout)
    Err(e) => {
      stdout.drop_sync()
      Err(e)
    }
  }
}

///|
pub fn[E] decode_result_empty(
  result : InvocationResult,
) -> Result[Unit, ToolError[E]] {
  let drops = @model_host.resource_drops()
  match expect_no_stdout(result.stdout) {
    Err(e) => {
      release_optional_value(result.result, drops)
      Err(e)
    }
    Ok(_) => expect_no_value_with_resource_drops(result.result, drops)
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
    Some(stdout) => {
      stdout.drop_sync()
      Err(
        tool_protocol_error("tool result unexpectedly contained stdout stream"),
      )
    }
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
  expect_no_value_with_resource_drops(value, @model_host.resource_drops())
}

///|
fn[E] expect_no_value_with_resource_drops(
  value : @model.TypedSchemaValue?,
  drops : @model.SchemaValueResourceDrops,
) -> Result[Unit, ToolError[E]] {
  match value {
    Some(value) => {
      value.value.release_resources(drops)
      Err(tool_protocol_error("tool result unexpectedly contained a value"))
    }
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
  drops : @model.SchemaValueResourceDrops,
) -> Result[T, ToolError[E]] {
  let value = match expect_value(value) {
    Ok(value) => value
    Err(e) => return Err(e)
  }
  try @schema.from_value_as(value.value) catch {
    e => {
      value.value.release_resources(drops)
      Err(tool_protocol_error("failed to decode tool result: \{Repr(e)}"))
    }
  } noraise {
    decoded => Ok(decoded)
  }
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
      let decoded = @model_host.typed_schema_value_from_wit(value) catch {
        e =>
          return tool_protocol_error(
            "failed to decode remote tool error: \{Repr(e)}",
          )
      }
      match decode_error(decoded) {
        Ok(error) => ToolError::Tool(error)
        Err(message) => {
          decoded.value.release_resources(@model_host.resource_drops())
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
