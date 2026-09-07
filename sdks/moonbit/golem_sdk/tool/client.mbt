///|
pub(all) enum RpcError {
  Protocol(String)
  Denied(String)
  NotFound(String)
  RemoteInternal(String)
  Cancelled
  ResourceExhausted(String)
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
priv enum CollectedStdout {
  Collected(Bytes?)
  Failed(Error)
}

///|
pub struct ToolInvocation[E] {
  priv future : @toolHost.FutureInvokeResult
  priv decode_error : (@model.TypedSchemaValue) -> Result[E, String]
  priv stdout_source : @async-core.Stream[
    Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
  ]?
  stdout : @async-core.Stream[Byte]?
}

///|
pub struct TypedToolInvocation[T, E] {
  priv invocation : ToolInvocation[E]
  priv decode : (InvocationResult) -> Result[T, ToolError[E]]
  stdout : @async-core.Stream[Byte]
}

///|
pub fn[T, E] typed_invocation(
  invocation : ToolInvocation[E],
  decode : (InvocationResult) -> Result[T, ToolError[E]],
) -> Result[TypedToolInvocation[T, E], ToolError[E]] {
  match invocation.stdout {
    None =>
      Err(tool_protocol_error("tool invocation did not create declared stdout"))
    Some(stdout) => Ok({ invocation, decode, stdout, })
  }
}

///|
pub fn[T, E] TypedToolInvocation::cancel(
  self : TypedToolInvocation[T, E],
) -> Unit {
  self.invocation.cancel()
}

///|
pub fn[T, E] TypedToolInvocation::drop(
  self : TypedToolInvocation[T, E],
) -> Unit {
  self.invocation.drop()
}

///|
pub async fn[T, E] TypedToolInvocation::get(
  self : TypedToolInvocation[T, E],
) -> Result[T, ToolError[E]] {
  match self.invocation.get() {
    Err(error) => Err(error)
    Ok(result) => (self.decode)(result)
  }
}

///|
pub async fn[T, E] TypedToolInvocation::collect(
  self : TypedToolInvocation[T, E],
) -> Result[(T, Bytes), ToolError[E]] {
  match self.invocation.collect() {
    Err(error) => Err(error)
    Ok((result, Some(bytes))) =>
      match (self.decode)(result) {
        Ok(value) => Ok((value, bytes))
        Err(error) => Err(error)
      }
    Ok((_, None)) =>
      Err(tool_protocol_error("declared stdout was not collected"))
  }
}

///|
pub fn[E] ToolInvocation::cancel(self : ToolInvocation[E]) -> Unit {
  self.future.cancel()
}

///|
pub fn[E] ToolInvocation::drop(self : ToolInvocation[E]) -> Unit {
  self.future.drop()
  match self.stdout_source {
    Some(source) => source.drop_sync()
    None => ()
  }
  match self.stdout {
    Some(stdout) => stdout.drop_sync()
    None => ()
  }
}

///|
pub async fn[E] ToolInvocation::get(
  self : ToolInvocation[E],
) -> Result[InvocationResult, ToolError[E]] {
  match self.future.get() {
    Ok(result) => decode_invocation_result(result)
    Err(error) => Err(map_rpc_error(error, self.decode_error))
  }
}

///|
pub async fn[E] ToolInvocation::collect(
  self : ToolInvocation[E],
) -> Result[(InvocationResult, Bytes?), ToolError[E]] {
  @async-core.with_task_group(async fn(group) {
    let output = group.spawn(async fn() {
      try {
        let collected = match self.stdout {
          None => None
          Some(stream) => {
            let bytes : Array[Byte] = []
            for ;; {
              match stream.read(65536) {
                None => break
                Some(chunk) => bytes.append(chunk)
              }
            }
            Some(Bytes::from_array(bytes))
          }
        }
        CollectedStdout::Collected(collected)
      } catch {
        error => CollectedStdout::Failed(error)
      }
    })
    let result = self.get()
    let output = output.wait()
    select_collected_terminals(result, output)
  })
}

///|
fn[E] select_collected_terminals(
  result : Result[InvocationResult, ToolError[E]],
  output : CollectedStdout,
) -> Result[(InvocationResult, Bytes?), ToolError[E]] raise {
  match result {
    Err(error) => Err(error)
    Ok(result) =>
      match output {
        Collected(bytes) => Ok((result, bytes))
        Failed(error) => raise error
      }
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
pub fn[E] ToolClient::start(
  self : ToolClient,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  stdout : Bool,
  decode_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> Result[ToolInvocation[E], ToolError[E]] {
  let wire_input = @model.typed_schema_value_to_wit(input) catch {
    e => {
      release_failed_tool_input(input, @model_host.resource_drops())
      match stdin {
        Some(stdin) => stdin.drop_sync()
        None => ()
      }
      return Err(tool_protocol_error("failed to encode tool input: \{Repr(e)}"))
    }
  }
  let wire_stdin = prepare_stdin(stdin)
  let (wire_stdout, output, stdout_source) = if stdout {
    let (target, stream) = @toolHost.create_stdout()
    (Some(target), Some(invocation_stdout(stream)), Some(stream))
  } else {
    (None, None, None)
  }
  let future = self.rpc.async_invoke_and_await(
    command_path, wire_input, wire_stdin, wire_stdout,
  )
  Ok({ future, decode_error, stdout_source, stdout: output, })
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
    @async-core.Cancelled::Cancelled as error => {
      release_failed_tool_input(input, @model_host.resource_drops())
      match stdin {
        Some(stdin) => stdin.drop_sync()
        None => ()
      }
      raise error
    }
    e => {
      release_failed_tool_input(input, @model_host.resource_drops())
      match stdin {
        Some(stdin) => stdin.drop_sync()
        None => ()
      }
      return Err(tool_protocol_error("failed to encode tool input: \{Repr(e)}"))
    }
  }
  let wire_stdin = prepare_stdin(stdin)
  let wire_result = rpc.invoke_and_await(
    command_path,
    wire_input,
    wire_stdin,
    None,
  )
  let wire_result = match wire_result {
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
  let drops = @model_host.resource_drops()
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
  let drops = @model_host.resource_drops()
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
    @toolHost.RpcError::Cancelled => ToolError::Rpc(RpcError::Cancelled)
    @toolHost.RpcError::ResourceExhausted(message) =>
      ToolError::Rpc(RpcError::ResourceExhausted(message))
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
