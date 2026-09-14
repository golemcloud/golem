///|
pub(all) suberror ToolStreamError {
  Cancelled
  Abandoned
  ResourceExhausted
  Failed(String)
} derive(Debug, Eq)

///|
fn tool_stream_failure(error : ToolStreamError) -> @toolHost.ByteStreamFailure {
  match error {
    ToolStreamError::Cancelled => @toolHost.ByteStreamFailure::Cancelled
    ToolStreamError::Abandoned => @toolHost.ByteStreamFailure::Abandoned
    ToolStreamError::ResourceExhausted =>
      @toolHost.ByteStreamFailure::ResourceExhausted
    ToolStreamError::Failed(message) =>
      @toolHost.ByteStreamFailure::Failed(message)
  }
}

///|
priv struct ProviderStdoutState {
  write_callback : async (FixedArray[Byte]) -> Result[
    Unit,
    @toolHost.StreamWriteError,
  ]
  finish_callback : async () -> Result[Unit, @toolHost.StreamWriteError]
  fail_callback : async (@toolHost.ByteStreamFailure) -> Result[
    Unit,
    @toolHost.StreamWriteError,
  ]
  drop_callback : () -> Unit
  mut open : Bool
  mut busy : Bool
  mut dropped : Bool
}

///|
pub struct ProviderStdout {
  priv state : Ref[ProviderStdoutState]
  priv output : @async-core.Sink[Byte]
}

///|
fn provider_stdout_from_callbacks(
  write_callback : async (FixedArray[Byte]) -> Result[
    Unit,
    @toolHost.StreamWriteError,
  ],
  finish_callback : async () -> Result[Unit, @toolHost.StreamWriteError],
  fail_callback : async (@toolHost.ByteStreamFailure) -> Result[
    Unit,
    @toolHost.StreamWriteError,
  ],
  drop_callback : () -> Unit,
) -> ProviderStdout {
  let state = @ref.new({
    write_callback,
    finish_callback,
    fail_callback,
    drop_callback,
    open: true,
    busy: false,
    dropped: false,
  })
  let output = @async-core.Sink::from_callbacks(
    async fn(bytes) {
      if !state.val.open || state.val.dropped {
        return 0
      }
      if state.val.busy {
        raise @toolHost.StreamWriteError::ConcurrentOperation
      }
      state.val.busy = true
      defer {
        state.val.busy = false
      }
      let owned = FixedArray::from_array(bytes)
      match (state.val.write_callback)(owned) {
        Ok(_) => bytes.length()
        Err(error) => {
          if error is @toolHost.StreamWriteError::Closed(_) {
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
        raise @toolHost.StreamWriteError::ConcurrentOperation
      }
      state.val.busy = true
      defer {
        state.val.busy = false
      }
      match (state.val.finish_callback)() {
        Ok(_) => state.val.open = false
        Err(error) => {
          if error is @toolHost.StreamWriteError::Closed(_) {
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
pub fn provider_stdout(writer : @toolHost.ToolStdoutWriter) -> ProviderStdout {
  provider_stdout_from_callbacks(
    bytes => writer.write(bytes),
    () => writer.finish(),
    reason => writer.fail(reason),
    () => writer.drop(),
  )
}

///|
pub async fn ProviderStdout::write(
  self : ProviderStdout,
  data : ArrayView[Byte],
) -> Int {
  self.output.write(data)
}

///|
pub async fn ProviderStdout::write_bytes(
  self : ProviderStdout,
  data : BytesView,
) -> Int {
  self.output.write_bytes(data)
}

///|
pub async fn ProviderStdout::write_all(
  self : ProviderStdout,
  data : ArrayView[Byte],
) -> Bool {
  self.output.write_all(data)
}

///|
pub async fn ProviderStdout::write_all_bytes(
  self : ProviderStdout,
  data : BytesView,
) -> Bool {
  self.output.write_all_bytes(data)
}

///|
pub fn ProviderStdout::is_open(self : ProviderStdout) -> Bool {
  self.state.val.open && !self.state.val.dropped
}

///|
pub async fn ProviderStdout::finish(self : ProviderStdout) -> Unit {
  if !self.state.val.open || self.state.val.dropped {
    return
  }
  if self.state.val.busy {
    raise @toolHost.StreamWriteError::ConcurrentOperation
  }
  self.state.val.busy = true
  defer {
    self.state.val.busy = false
  }
  match (self.state.val.finish_callback)() {
    Ok(_) => self.state.val.open = false
    Err(@toolHost.StreamWriteError::Closed(_)) => self.state.val.open = false
    Err(error) => raise error
  }
}

///|
pub async fn ProviderStdout::fail(
  self : ProviderStdout,
  reason : ToolStreamError,
) -> Unit {
  if !self.state.val.open || self.state.val.dropped {
    return
  }
  if self.state.val.busy {
    raise @toolHost.StreamWriteError::ConcurrentOperation
  }
  self.state.val.busy = true
  defer {
    self.state.val.busy = false
  }
  match (self.state.val.fail_callback)(tool_stream_failure(reason)) {
    Ok(_) => self.state.val.open = false
    Err(@toolHost.StreamWriteError::Closed(_)) => self.state.val.open = false
    Err(error) => raise error
  }
}

///|
pub fn ProviderStdout::drop(self : ProviderStdout) -> Unit {
  if !self.state.val.dropped {
    self.state.val.dropped = true
    self.state.val.open = false
    (self.state.val.drop_callback)()
  }
}
