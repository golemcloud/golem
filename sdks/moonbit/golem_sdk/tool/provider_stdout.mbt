///|
fn tool_stream_failure(
  error : @toolCore.ToolStreamError,
) -> @toolStreams.ByteStreamFailure {
  match error {
    Cancelled => @toolStreams.ByteStreamFailure::Cancelled
    Abandoned => @toolStreams.ByteStreamFailure::Abandoned
    ResourceExhausted => @toolStreams.ByteStreamFailure::ResourceExhausted
    Failed(message) => @toolStreams.ByteStreamFailure::Failed(message)
  }
}

///|
priv struct ProviderOutputState {
  write_callback : async (FixedArray[Byte]) -> Result[
    Unit,
    @toolStreams.StreamWriteError,
  ]
  finish_callback : async () -> Result[Unit, @toolStreams.StreamWriteError]
  fail_callback : async (@toolStreams.ByteStreamFailure) -> Result[
    Unit,
    @toolStreams.StreamWriteError,
  ]
  drop_callback : () -> Unit
  mut open : Bool
  mut busy : Bool
  mut dropped : Bool
  mut closing : Bool
  mut idle : @async-core.Future[Unit]
  mut idle_promise : @async-core.Promise[Unit]?
}

///|
pub struct ProviderOutput {
  priv state : Ref[ProviderOutputState]
  priv output : @async-core.Sink[Byte]
}

///|
fn provider_output_from_callbacks(
  write_callback : async (FixedArray[Byte]) -> Result[
    Unit,
    @toolStreams.StreamWriteError,
  ],
  finish_callback : async () -> Result[Unit, @toolStreams.StreamWriteError],
  fail_callback : async (@toolStreams.ByteStreamFailure) -> Result[
    Unit,
    @toolStreams.StreamWriteError,
  ],
  drop_callback : () -> Unit,
) -> ProviderOutput {
  let state = @ref.new({
    write_callback,
    finish_callback,
    fail_callback,
    drop_callback,
    open: true,
    busy: false,
    dropped: false,
    closing: false,
    idle: @async-core.Future::ready(()),
    idle_promise: None,
  })
  let output = @async-core.Sink::from_callbacks(
    async fn(bytes) {
      if !state.val.open || state.val.dropped {
        return 0
      }
      match (state.val.write_callback)(FixedArray::from_array(bytes)) {
        Ok(_) => bytes.length()
        Err(error) => {
          if error is @toolStreams.StreamWriteError::Closed(_) {
            state.val.open = false
          }
          raise error
        }
      }
    },
    async fn() {
      if !state.val.open || state.val.dropped {
        return
      }
      if state.val.busy {
        raise @toolStreams.StreamWriteError::ConcurrentOperation
      }
      state.val.begin_operation()
      defer state.val.end_operation()
      match (state.val.finish_callback)() {
        Ok(_) => state.val.open = false
        Err(error) => {
          if error is @toolStreams.StreamWriteError::Closed(_) {
            state.val.open = false
          }
          raise error
        }
      }
    },
    fn() { state.val.open && !state.val.dropped },
    None,
  )
  { state, output, }
}

///|
fn ProviderOutputState::begin_operation(self : ProviderOutputState) -> Unit {
  let (idle, promise) = @async-core.Future::new()
  self.idle = idle
  self.idle_promise = Some(promise)
  self.busy = true
}

///|
fn ProviderOutput::begin_write(self : ProviderOutput) -> Unit raise {
  if self.state.val.busy || self.state.val.closing {
    raise @toolStreams.StreamWriteError::ConcurrentOperation
  }
  self.state.val.begin_operation()
}

///|
fn ProviderOutputState::end_operation(self : ProviderOutputState) -> Unit {
  self.busy = false
  if self.dropped {
    (self.drop_callback)()
  }
  if self.idle_promise is Some(promise) {
    self.idle_promise = None
    ignore(promise.complete(()))
  }
}

///|
pub fn provider_output(
  writer : @toolStreams.ToolOutputWriter,
) -> ProviderOutput {
  provider_output_from_callbacks(
    bytes => writer.write(bytes),
    () => writer.finish(),
    reason => writer.fail(reason),
    () => writer.drop(),
  )
}

///|
pub async fn ProviderOutput::write(
  self : ProviderOutput,
  data : ArrayView[Byte],
) -> Int {
  if !self.is_open() {
    return 0
  }
  self.begin_write()
  defer self.state.val.end_operation()
  self.output.write(data)
}

///|
pub async fn ProviderOutput::write_bytes(
  self : ProviderOutput,
  data : BytesView,
) -> Int {
  if !self.is_open() {
    return 0
  }
  self.begin_write()
  defer self.state.val.end_operation()
  self.output.write_bytes(data)
}

///|
pub async fn ProviderOutput::write_all(
  self : ProviderOutput,
  data : ArrayView[Byte],
) -> Bool {
  if !self.is_open() {
    return data.is_empty()
  }
  self.begin_write()
  defer self.state.val.end_operation()
  self.output.write_all(data)
}

///|
pub async fn ProviderOutput::write_all_bytes(
  self : ProviderOutput,
  data : BytesView,
) -> Bool {
  if !self.is_open() {
    return data.is_empty()
  }
  self.begin_write()
  defer self.state.val.end_operation()
  self.output.write_all_bytes(data)
}

///|
pub fn ProviderOutput::is_open(self : ProviderOutput) -> Bool {
  self.state.val.open && !self.state.val.dropped
}

///|
pub async fn ProviderOutput::finish(self : ProviderOutput) -> Unit {
  if !self.state.val.open || self.state.val.dropped {
    return
  }
  if self.state.val.busy {
    raise @toolStreams.StreamWriteError::ConcurrentOperation
  }
  self.state.val.begin_operation()
  defer self.state.val.end_operation()
  match (self.state.val.finish_callback)() {
    Ok(_) => self.state.val.open = false
    Err(@toolStreams.StreamWriteError::Closed(cause)) => {
      self.state.val.open = false
      raise @toolStreams.StreamWriteError::Closed(cause)
    }
    Err(error) => raise error
  }
}

///|
pub async fn ProviderOutput::fail(
  self : ProviderOutput,
  reason : @toolCore.ToolStreamError,
) -> Unit {
  if !self.state.val.open || self.state.val.dropped {
    return
  }
  if self.state.val.busy {
    raise @toolStreams.StreamWriteError::ConcurrentOperation
  }
  self.state.val.begin_operation()
  defer self.state.val.end_operation()
  match (self.state.val.fail_callback)(tool_stream_failure(reason)) {
    Ok(_) => self.state.val.open = false
    Err(@toolStreams.StreamWriteError::Closed(cause)) => {
      self.state.val.open = false
      raise @toolStreams.StreamWriteError::Closed(cause)
    }
    Err(error) => raise error
  }
}

///|
pub fn ProviderOutput::drop(self : ProviderOutput) -> Unit {
  if !self.state.val.dropped {
    self.state.val.dropped = true
    self.state.val.open = false
    if !self.state.val.busy {
      (self.state.val.drop_callback)()
    }
  }
}

///|
async fn ProviderOutput::complete(
  self : ProviderOutput,
  failure : ToolStreamError?,
) -> Unit {
  self.state.val.closing = true
  self.state.val.idle.get()
  match failure {
    Some(reason) => self.fail(reason)
    None => self.finish()
  }
}

///|
#doc(hidden)
pub async fn invoke_with_output(
  output : ProviderOutput,
  invoke : async () -> Result[@toolCommon.InvocationResult, @types.ToolError],
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  defer output.drop()
  let result = invoke() catch {
    error => {
      let reason = match error {
        @async-core.Cancelled::Cancelled => ToolStreamError::Cancelled
        _ => ToolStreamError::Failed(error.to_string())
      }
      @async-core.protect_from_cancel(
        () => output.complete(Some(reason)) catch { _ => () },
        resume_on_cancel=true,
      )
      raise error
    }
  }
  let reason = match result {
    Ok(_) | Err(@types.ToolError::CustomError(_)) => None
    Err(_) => Some(ToolStreamError::Failed("tool invocation failed"))
  }
  @async-core.protect_from_cancel(() => output.complete(reason)) catch {
    error =>
      match (error, result) {
        (@async-core.Cancelled::Cancelled, Ok(value)) => {
          match @toolCore.decode_invocation_result(value) {
            Ok(decoded) => @toolCore.drop_invocation_result(decoded)
            Err(_) => ()
          }
          raise error
        }
        (
          @async-core.Cancelled::Cancelled,
          Err(@types.ToolError::CustomError(payload)),
        ) => {
          match @toolCore.decode_typed_value(payload.payload, "tool error") {
            Ok(decoded) => @model.drop_owned_capabilities(decoded.value)
            Err(_) => ()
          }
          raise error
        }
        (@async-core.Cancelled::Cancelled, Err(_)) => raise error
        _ => ()
      }
  }
  result
}
