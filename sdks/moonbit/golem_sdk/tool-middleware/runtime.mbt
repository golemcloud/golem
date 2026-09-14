///|
pub(all) enum ToolInvokeError[E] {
  InvalidToolName(String)
  InvalidCommandPath(Array[String])
  InvalidInput(String)
  ConstraintViolation(String)
  InvalidResult(String)
  Tool(E)
} derive(Eq, Debug)

///|
pub fn[E, M] ToolInvokeError::map_tool(
  self : ToolInvokeError[E],
  transform : (E) -> M,
) -> ToolInvokeError[M] {
  match self {
    InvalidToolName(name) => InvalidToolName(name)
    InvalidCommandPath(path) => InvalidCommandPath(path)
    InvalidInput(message) => InvalidInput(message)
    ConstraintViolation(message) => ConstraintViolation(message)
    InvalidResult(message) => InvalidResult(message)
    Tool(error) => Tool(transform(error))
  }
}

///|
pub struct UnderlyingTool {
  priv ownership : OwnershipContext
  priv invoke : async (
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Byte]?,
  ) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError]
}

///|
fn new_underlying_tool(
  invoke : async (
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Byte]?,
  ) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError],
  drop_raw : () -> Unit,
) -> UnderlyingTool {
  { ownership: OwnershipContext::new(drop_raw), invoke, }
}

///|
fn new_scoped_underlying_tool(
  invoke : async (
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Byte]?,
  ) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError],
  drop_raw : () -> Unit,
  background_group : @async-core.TaskGroup[Unit],
) -> UnderlyingTool {
  let underlying = new_underlying_tool(invoke, drop_raw)
  background_group.add_defer(() => underlying.ownership.revoke())
  underlying
}

///|
#doc(hidden)
pub fn UnderlyingTool::from_wit(
  raw : @toolCommon.UnderlyingTool,
  background_group : @async-core.TaskGroup[Unit],
) -> UnderlyingTool {
  new_scoped_underlying_tool(
    async fn(command_path, input, stdin) {
      raw.invoke(command_path, input, stdin)
    },
    fn() { raw.drop() },
    background_group,
  )
}

///|
#doc(hidden)
pub fn UnderlyingTool::ownership_context(
  self : UnderlyingTool,
) -> OwnershipContext {
  self.ownership
}

///|
#doc(hidden)
pub fn UnderlyingTool::revoke(self : UnderlyingTool) -> Unit {
  self.ownership.revoke()
}

///|
pub async fn UnderlyingTool::invoke(
  self : UnderlyingTool,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : MiddlewareStdin?,
) -> Result[OwnedInvocationResult, ToolInvokeError[@model.TypedSchemaValue]] {
  self.invoke_with(command_path, input, stdin, value => Ok(value))
}

///|
#doc(hidden)
pub async fn[E] UnderlyingTool::invoke_with(
  self : UnderlyingTool,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : MiddlewareStdin?,
  decode_custom_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> Result[OwnedInvocationResult, ToolInvokeError[E]] {
  match self.ownership.begin_call() {
    Err(error) => return Err(error)
    Ok(_) => ()
  }
  defer self.ownership.finish_call()
  match stdin {
    Some(stdin) =>
      match stdin.check_for_underlying(self.ownership) {
        Ok(_) => ()
        Err(error) => return Err(InvalidInput(error))
      }
    None => ()
  }
  let wire_input = match @toolCore.encode_typed_input(input) {
    Ok(encoded) => encoded
    Err(error) => return Err(InvalidInput(error))
  }
  let wire_stdin = match stdin {
    Some(stdin) =>
      match stdin.take_for_underlying() {
        Ok(stream) => Some(stream)
        Err(error) => return Err(InvalidInput(error))
      }
    None => None
  }
  let wire_result = match
    (self.invoke)(command_path.copy(), wire_input, wire_stdin) {
    Ok(result) => result
    Err(error) =>
      return Err(
        decode_tool_error_with_ownership(
          error,
          decode_custom_error,
          Some(self.ownership),
        ),
      )
  }
  match @toolCore.decode_invocation_result(wire_result) {
    Ok(result) => Ok(self.ownership.own_result(result))
    Err(error) => Err(InvalidResult(error))
  }
}

///|
pub fn[E] decode_tool_error(
  error : @toolCommon.ToolError,
  decode_custom_error : (@model.TypedSchemaValue) -> Result[E, String],
) -> ToolInvokeError[E] {
  decode_tool_error_with_ownership(error, decode_custom_error, None)
}

///|
fn[E] decode_tool_error_with_ownership(
  error : @toolCommon.ToolError,
  decode_custom_error : (@model.TypedSchemaValue) -> Result[E, String],
  ownership : OwnershipContext?,
) -> ToolInvokeError[E] {
  match error {
    @toolCommon.ToolError::InvalidToolName(name) => InvalidToolName(name)
    @toolCommon.ToolError::InvalidCommandPath(path) => InvalidCommandPath(path)
    @toolCommon.ToolError::InvalidInput(message) => InvalidInput(message)
    @toolCommon.ToolError::ConstraintViolation(message) =>
      ConstraintViolation(message)
    @toolCommon.ToolError::InvalidResult(message) => InvalidResult(message)
    @toolCommon.ToolError::CustomError(value) => {
      let decoded = match @toolCore.decode_typed_value(value, "tool error") {
        Ok(decoded) => decoded
        Err(message) => return InvalidResult(message)
      }
      match decode_custom_error(decoded) {
        Ok(error) => {
          match ownership {
            Some(ownership) =>
              ownership.register_cleanup(fn() {
                @model.drop_owned_capabilities(decoded.value)
              })
            None => ()
          }
          Tool(error)
        }
        Err(message) => {
          @model.drop_owned_capabilities(decoded.value)
          InvalidResult(message)
        }
      }
    }
  }
}

///|
pub fn encode_tool_error(
  error : ToolInvokeError[@model.TypedSchemaValue],
) -> @toolCommon.ToolError {
  match error {
    InvalidToolName(name) => @toolCommon.ToolError::InvalidToolName(name)
    InvalidCommandPath(path) => @toolCommon.ToolError::InvalidCommandPath(path)
    InvalidInput(message) => @toolCommon.ToolError::InvalidInput(message)
    ConstraintViolation(message) =>
      @toolCommon.ToolError::ConstraintViolation(message)
    InvalidResult(message) => @toolCommon.ToolError::InvalidResult(message)
    Tool(value) => {
      let encoded = @model.typed_schema_value_to_wit(value) catch {
        error => {
          @model.drop_owned_capabilities(value.value)
          return @toolCommon.ToolError::InvalidResult(
            "failed to encode tool error: \{Repr(error)}",
          )
        }
      }
      @toolCommon.ToolError::CustomError(encoded)
    }
  }
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_with_stdout(
  result : OwnedInvocationResult,
) -> Result[(T, @async-core.Stream[Byte]), ToolInvokeError[E]] {
  let result = match result.take_for_projection() {
    Ok(result) => result
    Err(message) => return Err(InvalidResult(message))
  }
  match @toolCore.project_result_with_stdout(result) {
    Ok(projected) => Ok(projected)
    Err(message) => Err(InvalidResult(message))
  }
}

///|
pub fn[T : @schema.FromSchema, E] decode_result_value(
  result : OwnedInvocationResult,
) -> Result[T, ToolInvokeError[E]] {
  let result = match result.take_for_projection() {
    Ok(result) => result
    Err(message) => return Err(InvalidResult(message))
  }
  match @toolCore.project_result_value(result) {
    Ok(value) => Ok(value)
    Err(message) => Err(InvalidResult(message))
  }
}

///|
pub fn[E] decode_result_stdout_only(
  result : OwnedInvocationResult,
) -> Result[@async-core.Stream[Byte], ToolInvokeError[E]] {
  let result = match result.take_for_projection() {
    Ok(result) => result
    Err(message) => return Err(InvalidResult(message))
  }
  match @toolCore.project_result_stdout_only(result) {
    Ok(stdout) => Ok(stdout)
    Err(message) => Err(InvalidResult(message))
  }
}

///|
pub fn[E] decode_result_empty(
  result : OwnedInvocationResult,
) -> Result[Unit, ToolInvokeError[E]] {
  let result = match result.take_for_projection() {
    Ok(result) => result
    Err(message) => return Err(InvalidResult(message))
  }
  match @toolCore.project_result_empty(result) {
    Ok(_) => Ok(())
    Err(message) => Err(InvalidResult(message))
  }
}
