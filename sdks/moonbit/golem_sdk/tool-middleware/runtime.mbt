///|
pub(all) enum ToolInvokeError[E] {
  InvalidToolName(String)
  InvalidCommandPath(Array[String])
  InvalidInput(String)
  ConstraintViolation(String)
  InvalidResult(String)
  Tool(E)
  UnknownToolError(@types.CustomToolError)
  ProtocolError(String)
  Denied(String)
  InternalError(String)
  Cancelled
  ResourceExhausted(String)
} derive(Eq, Debug)

///|
pub(all) struct RawCustomToolError {
  name : String
  payload : @model.TypedSchemaValue
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
    UnknownToolError(error) => UnknownToolError(error)
    ProtocolError(message) => ProtocolError(message)
    Denied(message) => Denied(message)
    InternalError(message) => InternalError(message)
    Cancelled => Cancelled
    ResourceExhausted(message) => ResourceExhausted(message)
  }
}

///|
pub struct UnderlyingTool {
  priv ownership : OwnershipContext
  priv invoke : async (
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Result[FixedArray[Byte], @toolStreams.ByteStreamFailure]]?,
  ) -> UnderlyingObserver
}

///|
priv struct UnderlyingObserver {
  get : async () -> Result[
    @toolCommon.InvocationResult,
    @underlying.UnderlyingError,
  ]
  stdout : @async-core.Stream[Byte]?
  cancel : () -> Unit
  drop : () -> Unit
}

///|
/// One admitted invocation of the next tool in a middleware chain. Its stdout
/// can be consumed while `get` is still waiting for the result. Dropping the
/// observer only releases observation; cancellation is always explicit.
pub struct UnderlyingInvocation[T, E] {
  priv get_result : async () -> Result[T, ToolInvokeError[E]]
  priv stdout_stream : @async-core.Stream[Byte]?
  priv cancel_call : () -> Unit
  priv drop_observer : () -> Unit
  priv state : Ref[UnderlyingInvocationState]
  priv result_lock : @async-core.Mutex
  priv cached_result : Ref[Result[T, ToolInvokeError[E]]?]
}

///|
priv struct UnderlyingInvocationState {
  mut open : Bool
  mut get_in_flight : Bool
  mut raw_dropped : Bool
}

///|
pub async fn[T, E] UnderlyingInvocation::get(
  self : UnderlyingInvocation[T, E],
) -> Result[T, ToolInvokeError[E]] {
  self.result_lock.acquire()
  defer self.result_lock.release()
  if self.cached_result.val is Some(result) {
    return result
  }
  @async-core.protect_from_cancel(async fn() {
    let result = (self.get_result)()
    self.cached_result.val = Some(result)
    result
  })
}

///|
pub fn[T, E] UnderlyingInvocation::stdout(
  self : UnderlyingInvocation[T, E],
) -> @async-core.Stream[Byte]? {
  self.stdout_stream
}

///|
pub fn[T, E] UnderlyingInvocation::cancel(
  self : UnderlyingInvocation[T, E],
) -> Unit {
  if self.state.val.open {
    (self.cancel_call)()
  }
}

///|
pub fn[T, E] UnderlyingInvocation::drop(
  self : UnderlyingInvocation[T, E],
) -> Unit {
  (self.drop_observer)()
}

///|
fn new_underlying_tool(
  invoke : async (
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Byte]?,
  ) -> Result[@toolCommon.InvocationResult, @types.ToolError],
  drop_raw : () -> Unit,
) -> UnderlyingTool {
  let ownership = OwnershipContext::new(drop_raw)
  {
    ownership,
    invoke: async fn(command_path, input, stdin) {
      let result = invoke(
        command_path,
        input,
        stdin.map(failure_aware_byte_stream),
      )
      {
        get: async fn() {
          result.map_err(error => @underlying.UnderlyingError::ToolError(error))
        },
        stdout: None,
        cancel: fn() {  },
        drop: fn() {  },
      }
    },
  }
}

///|
fn new_scoped_underlying_tool(
  invoke : async (
    Array[String],
    @types.TypedSchemaValue,
    @async-core.Stream[Byte]?,
  ) -> Result[@toolCommon.InvocationResult, @types.ToolError],
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
  raw : @underlying.UnderlyingTool,
  background_group : @async-core.TaskGroup[Unit],
) -> UnderlyingTool {
  let underlying : UnderlyingTool = {
    ownership: OwnershipContext::new(fn() { raw.drop() }),
    invoke: async fn(command_path, input, stdin) {
      let (observer, stdout) = raw.invoke(command_path, input, stdin)
      {
        get: async fn() {
          observer.get().map(value => { result: value, stdout: None, })
        },
        stdout: stdout.map(failure_aware_byte_stream),
        cancel: () => observer.cancel(),
        drop: () => observer.drop(),
      }
    },
  }
  background_group.add_defer(() => underlying.ownership.revoke())
  underlying
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
) -> Result[OwnedInvocationResult, ToolInvokeError[RawCustomToolError]] {
  self.invoke_with(command_path, input, stdin, fn(name, payload) {
    Some(Ok({ name, payload, }))
  })
}

///|
#doc(hidden)
pub async fn[E] UnderlyingTool::invoke_with(
  self : UnderlyingTool,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : MiddlewareStdin?,
  decode_custom_error : (String, @model.TypedSchemaValue) -> Result[E, String]?,
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
  let observer = (self.invoke)(command_path.copy(), wire_input, wire_stdin)
  defer (observer.drop)()
  let wire_result = match (observer.get)() {
    Ok(value) => {
      let combined : @toolCommon.InvocationResult = {
        result: value.result,
        stdout: match observer.stdout {
          Some(stdout) => Some(stdout)
          None => value.stdout
        },
      }
      combined
    }
    Err(@underlying.UnderlyingError::ToolError(error)) =>
      return Err(
        decode_tool_error_with_ownership(
          error,
          decode_custom_error,
          Some(self.ownership),
        ),
      )
    Err(@underlying.UnderlyingError::ProtocolError(message)) =>
      return Err(ProtocolError(message))
    Err(@underlying.UnderlyingError::Denied(message)) =>
      return Err(Denied(message))
    Err(@underlying.UnderlyingError::InternalError(message)) =>
      return Err(InternalError(message))
    Err(@underlying.UnderlyingError::Cancelled) => return Err(Cancelled)
    Err(@underlying.UnderlyingError::ResourceExhausted(message)) =>
      return Err(ResourceExhausted(message))
  }
  match @toolCore.decode_invocation_result(wire_result) {
    Ok(result) => Ok(self.ownership.own_result(result))
    Err(error) => Err(InvalidResult(error))
  }
}

///|
/// Starts one independent invocation and returns its observer as soon as the
/// child has been admitted. The projection runs only when `get` is called.
#doc(hidden)
pub async fn[T, E] UnderlyingTool::start_with(
  self : UnderlyingTool,
  command_path : Array[String],
  input : @model.TypedSchemaValue,
  stdin : MiddlewareStdin?,
  decode_custom_error : (String, @model.TypedSchemaValue) -> Result[E, String]?,
  project : (OwnedInvocationResult) -> Result[T, ToolInvokeError[E]],
) -> Result[UnderlyingInvocation[T, E], ToolInvokeError[E]] {
  match self.ownership.begin_call() {
    Err(error) => return Err(error)
    Ok(_) => ()
  }
  match stdin {
    Some(stdin) =>
      match stdin.check_for_underlying(self.ownership) {
        Ok(_) => ()
        Err(error) => {
          self.ownership.finish_call()
          return Err(InvalidInput(error))
        }
      }
    None => ()
  }
  let wire_input = match @toolCore.encode_typed_input(input) {
    Ok(encoded) => encoded
    Err(error) => {
      self.ownership.finish_call()
      return Err(InvalidInput(error))
    }
  }
  let wire_stdin = match stdin {
    Some(stdin) =>
      match stdin.take_for_underlying() {
        Ok(stream) => Some(stream)
        Err(error) => {
          self.ownership.finish_call()
          return Err(InvalidInput(error))
        }
      }
    None => None
  }
  let observer = (self.invoke)(command_path.copy(), wire_input, wire_stdin)
  let state = Ref({ open: true, get_in_flight: false, raw_dropped: false, })
  let finish_close = fn() {
    if !state.val.raw_dropped {
      state.val.raw_dropped = true
      (observer.drop)()
      self.ownership.finish_call()
    }
  }
  let close = fn() {
    if state.val.open {
      state.val.open = false
      if !state.val.get_in_flight {
        finish_close()
      }
    }
  }
  Ok({
    stdout_stream: observer.stdout,
    cancel_call: observer.cancel,
    drop_observer: close,
    state,
    result_lock: @async-core.Mutex(),
    cached_result: Ref(None),
    get_result: async fn() {
      if !state.val.open || state.val.get_in_flight {
        return Err(
          ProtocolError("underlying invocation observer is no longer available"),
        )
      }
      state.val.get_in_flight = true
      defer {
        state.val.get_in_flight = false
        state.val.open = false
        finish_close()
      }
      match (observer.get)() {
        Ok(wire_result) =>
          match @toolCore.decode_invocation_result(wire_result) {
            Ok(result) => project(self.ownership.own_result(result))
            Err(error) => Err(InvalidResult(error))
          }
        Err(@underlying.UnderlyingError::ToolError(error)) =>
          Err(
            decode_tool_error_with_ownership(
              error,
              decode_custom_error,
              Some(self.ownership),
            ),
          )
        Err(@underlying.UnderlyingError::ProtocolError(message)) =>
          Err(ProtocolError(message))
        Err(@underlying.UnderlyingError::Denied(message)) =>
          Err(Denied(message))
        Err(@underlying.UnderlyingError::InternalError(message)) =>
          Err(InternalError(message))
        Err(@underlying.UnderlyingError::Cancelled) => Err(Cancelled)
        Err(@underlying.UnderlyingError::ResourceExhausted(message)) =>
          Err(ResourceExhausted(message))
      }
    },
  })
}

///|
pub fn[E] decode_tool_error(
  error : @types.ToolError,
  decode_custom_error : (String, @model.TypedSchemaValue) -> Result[E, String]?,
) -> ToolInvokeError[E] {
  decode_tool_error_with_ownership(error, decode_custom_error, None)
}

///|
fn[E] decode_tool_error_with_ownership(
  error : @types.ToolError,
  decode_custom_error : (String, @model.TypedSchemaValue) -> Result[E, String]?,
  ownership : OwnershipContext?,
) -> ToolInvokeError[E] {
  match error {
    @types.ToolError::InvalidToolName(name) => InvalidToolName(name)
    @types.ToolError::InvalidCommandPath(path) => InvalidCommandPath(path)
    @types.ToolError::InvalidInput(message) => InvalidInput(message)
    @types.ToolError::ConstraintViolation(message) =>
      ConstraintViolation(message)
    @types.ToolError::InvalidResult(message) => InvalidResult(message)
    @types.ToolError::CustomError(error) => {
      let decoded = match
        @toolCore.decode_typed_value(error.payload, "tool error") {
        Ok(decoded) => decoded
        Err(message) => return InvalidResult(message)
      }
      match decode_custom_error(error.name, decoded) {
        Some(Ok(error)) => {
          match ownership {
            Some(ownership) =>
              ownership.register_cleanup(fn() {
                @model.drop_owned_capabilities(decoded.value)
              })
            None => ()
          }
          Tool(error)
        }
        Some(Err(message)) => {
          @model.drop_owned_capabilities(decoded.value)
          InvalidResult(message)
        }
        None => UnknownToolError(error)
      }
    }
  }
}

///|
pub fn encode_tool_error(
  error : ToolInvokeError[RawCustomToolError],
) -> @types.ToolError {
  match error {
    InvalidToolName(name) => @types.ToolError::InvalidToolName(name)
    InvalidCommandPath(path) => @types.ToolError::InvalidCommandPath(path)
    InvalidInput(message) => @types.ToolError::InvalidInput(message)
    ConstraintViolation(message) =>
      @types.ToolError::ConstraintViolation(message)
    InvalidResult(message) => @types.ToolError::InvalidResult(message)
    ProtocolError(message) =>
      @types.ToolError::InvalidResult("protocol error: \{message}")
    Denied(message) => @types.ToolError::ConstraintViolation(message)
    InternalError(message) =>
      @types.ToolError::InvalidResult("internal error: \{message}")
    Cancelled =>
      @types.ToolError::ConstraintViolation(
        "underlying invocation was cancelled",
      )
    ResourceExhausted(message) =>
      @types.ToolError::ConstraintViolation(
        "underlying invocation exhausted resources: \{message}",
      )
    Tool(error) => {
      let encoded = @model.typed_schema_value_to_wit(error.payload) catch {
        encode_error => {
          @model.drop_owned_capabilities(error.payload.value)
          return @types.ToolError::InvalidResult(
            "failed to encode tool error: \{Repr(encode_error)}",
          )
        }
      }
      @types.ToolError::CustomError({ name: error.name, payload: encoded, })
    }
    UnknownToolError(error) => @types.ToolError::CustomError(error)
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
