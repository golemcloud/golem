///|
pub(all) suberror ToolStreamError {
  Cancelled
  Abandoned
  ResourceExhausted
  Failed(String)
} derive(Debug, Eq)

///|
fn stream_failure(error : @toolHost.ByteStreamFailure) -> ToolStreamError {
  match error {
    @toolHost.ByteStreamFailure::Cancelled => ToolStreamError::Cancelled
    @toolHost.ByteStreamFailure::Abandoned => ToolStreamError::Abandoned
    @toolHost.ByteStreamFailure::ResourceExhausted =>
      ToolStreamError::ResourceExhausted
    @toolHost.ByteStreamFailure::Failed(message) =>
      ToolStreamError::Failed(message)
  }
}

///|
priv struct ProviderStdoutState {
  writer : @toolHost.ToolStdoutWriter
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
pub fn provider_stdout(writer : @toolHost.ToolStdoutWriter) -> ProviderStdout {
  let state = @ref.new({ writer, open: true, busy: false, dropped: false })
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
      match state.val.writer.write(owned) {
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
      match state.val.writer.finish() {
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
  { state, output }
}

///|
pub fn ProviderStdout::output(self : ProviderStdout) -> @async-core.Sink[Byte] {
  self.output
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
  match self.state.val.writer.finish() {
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
    self.state.val.writer.drop()
  }
}

///|
pub fn byte_stream(
  source : @async-core.Stream[
    Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
  ],
) -> @async-core.Stream[Byte] {
  @async-core.Stream::produce(async fn(sink) {
    for ;; {
      match source.read(1) {
        None => break
        Some(items) =>
          for item in items {
            match item {
              Ok(chunk) =>
                if !sink.write_all(chunk) {
                  source.drop()
                  return
                }
              Err(error) => raise stream_failure(error)
            }
          }
      }
    }
  })
}

///|
fn stdin_byte_stream(
  source : @async-core.Stream[Byte],
) -> @async-core.Stream[Result[FixedArray[Byte], @toolHost.ByteStreamFailure]] {
  @async-core.Stream::produce(
    async fn(sink) {
      for ;; {
        let chunk = source.read(65536) catch {
          error => {
            let failure : Result[FixedArray[Byte], @toolHost.ByteStreamFailure] = Err(
              @toolHost.ByteStreamFailure::Failed(repr(error)),
            )
            ignore(sink.write_all([failure]))
            return
          }
        }
        match chunk {
          None => return
          Some(bytes) => {
            let item : Result[FixedArray[Byte], @toolHost.ByteStreamFailure] = Ok(
              bytes,
            )
            if !sink.write_all([item]) {
              source.drop()
              return
            }
          }
        }
      }
    },
    on_unstarted_drop=fn() { source.drop_sync() },
  )
}

///|
fn prepare_stdin(source : @async-core.Stream[Byte]?) -> @toolHost.ToolStdin? {
  match source {
    None => None
    Some(source) =>
      Some(@toolHost.create_stdin_from_stream(stdin_byte_stream(source)))
  }
}

///|
fn invocation_stdout(
  source : @async-core.Stream[
    Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
  ],
) -> @async-core.Stream[Byte] {
  @async-core.Stream::produce(
    async fn(sink) {
      for ;; {
        match source.read(1) {
          None => break
          Some(items) =>
            for item in items {
              match item {
                Ok(chunk) =>
                  if !sink.write_all(chunk) {
                    source.drop()
                    return
                  }
                Err(error) => raise stream_failure(error)
              }
            }
        }
      }
    },
    on_unstarted_drop=fn() { source.drop_sync() },
  )
}
