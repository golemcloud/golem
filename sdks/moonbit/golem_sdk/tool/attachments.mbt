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
priv struct ToolByteStreamSource {
  source : @async-core.Stream[
    Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
  ]
  mut pending : FixedArray[Byte]?
  mut pending_offset : Int
  mut closed : Bool
}

///|
fn ToolByteStreamSource::read_pending(
  self : ToolByteStreamSource,
  count : Int,
) -> FixedArray[Byte]? {
  guard self.pending is Some(bytes) else { return None }
  let remaining = bytes.length() - self.pending_offset
  let length = if remaining < count { remaining } else { count }
  let result = FixedArray::makei(length, i => bytes[self.pending_offset + i])
  self.pending_offset += length
  if self.pending_offset == bytes.length() {
    self.pending = None
    self.pending_offset = 0
  }
  Some(result)
}

///|
async fn ToolByteStreamSource::read(
  self : ToolByteStreamSource,
  count : Int,
) -> FixedArray[Byte]? {
  if self.closed {
    return None
  }
  if count <= 0 {
    return Some([])
  }
  match self.read_pending(count) {
    Some(bytes) => return Some(bytes)
    None => ()
  }
  for ;; {
    match self.source.read(1) {
      None => {
        self.closed = true
        return None
      }
      Some(items) if items.length() == 0 => return Some([])
      Some(items) => {
        guard! items.length() == 1
        match items[0] {
          Err(error) => {
            self.closed = true
            self.source.drop()
            raise stream_failure(error)
          }
          Ok(bytes) if bytes.length() == 0 => continue
          Ok(bytes) => {
            self.pending = Some(bytes)
            return self.read_pending(count)
          }
        }
      }
    }
  }
}

///|
async fn ToolByteStreamSource::close(
  self : ToolByteStreamSource,
) -> Unit noraise {
  if !self.closed {
    self.closed = true
    self.source.drop()
  }
}

///|
fn ToolByteStreamSource::close_sync(self : ToolByteStreamSource) -> Unit {
  if !self.closed {
    self.closed = true
    self.source.drop_sync()
  }
}

///|
pub fn byte_stream(
  source : @async-core.Stream[
    Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
  ],
) -> @async-core.Stream[Byte] {
  let source = ToolByteStreamSource::{
    source,
    pending: None,
    pending_offset: 0,
    closed: false,
  }
  @async-core.Stream::from_source(
    count => source.read(count),
    () => source.close(),
    () => source.close_sync(),
  )
}

///|
async fn reject_stdin_source(
  source : @async-core.Stream[Byte],
  sink : @async-core.Sink[Result[FixedArray[Byte], @toolHost.ByteStreamFailure]],
  failure : @toolHost.ByteStreamFailure,
) -> Unit {
  @async-core.protect_from_cancel(
    async fn() {
      source.drop()
      let failure : Result[FixedArray[Byte], @toolHost.ByteStreamFailure] = Err(
        failure,
      )
      ignore(sink.write_all([failure]))
    },
    resume_on_cancel=true,
  )
}

///|
fn stdin_byte_stream(
  source : @async-core.Stream[Byte],
) -> @async-core.Stream[Result[FixedArray[Byte], @toolHost.ByteStreamFailure]] {
  @async-core.Stream::produce(
    async fn(sink) {
      for ;; {
        let chunk = source.read(65536) catch {
          ToolStreamError::Cancelled => {
            reject_stdin_source(
              source,
              sink,
              @toolHost.ByteStreamFailure::Cancelled,
            )
            return
          }
          ToolStreamError::Abandoned => {
            reject_stdin_source(
              source,
              sink,
              @toolHost.ByteStreamFailure::Abandoned,
            )
            return
          }
          ToolStreamError::ResourceExhausted => {
            reject_stdin_source(
              source,
              sink,
              @toolHost.ByteStreamFailure::ResourceExhausted,
            )
            return
          }
          ToolStreamError::Failed(message) => {
            reject_stdin_source(
              source,
              sink,
              @toolHost.ByteStreamFailure::Failed(message),
            )
            return
          }
          @async-core.Cancelled::Cancelled => {
            reject_stdin_source(
              source,
              sink,
              @toolHost.ByteStreamFailure::Cancelled,
            )
            return
          }
          error => {
            reject_stdin_source(
              source,
              sink,
              @toolHost.ByteStreamFailure::Failed(repr(error)),
            )
            return
          }
        }
        match chunk {
          None => return
          Some(bytes) if bytes.length() == 0 => continue
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
  byte_stream(source)
}
