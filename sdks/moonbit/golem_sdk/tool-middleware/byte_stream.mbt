///|
priv struct FailureAwareByteSource {
  source : @async-core.Stream[
    Result[FixedArray[Byte], @toolStreams.ByteStreamFailure],
  ]
  mut pending : FixedArray[Byte]?
  mut offset : Int
  mut closed : Bool
}

///|
async fn FailureAwareByteSource::read(
  self : FailureAwareByteSource,
  count : Int,
) -> FixedArray[Byte]? {
  if self.closed {
    return None
  }
  for ;; {
    if self.pending is Some(bytes) {
      let length = (bytes.length() - self.offset).min(count)
      let result = FixedArray::makei(length, i => bytes[self.offset + i])
      self.offset += length
      if self.offset == bytes.length() {
        self.pending = None
        self.offset = 0
      }
      return Some(result)
    }
    match self.source.read(1) {
      None => {
        self.closed = true
        return None
      }
      Some([Ok(bytes)]) if bytes.length() > 0 => self.pending = Some(bytes)
      Some([Ok(_)]) | Some([]) => continue
      Some([Err(failure)]) => {
        self.closed = true
        self.source.drop()
        match failure {
          @toolStreams.ByteStreamFailure::Cancelled =>
            raise @toolCore.ToolStreamError::Cancelled
          @toolStreams.ByteStreamFailure::Abandoned =>
            raise @toolCore.ToolStreamError::Abandoned
          @toolStreams.ByteStreamFailure::ResourceExhausted =>
            raise @toolCore.ToolStreamError::ResourceExhausted
          @toolStreams.ByteStreamFailure::Failed(message) =>
            raise @toolCore.ToolStreamError::Failed(message)
        }
      }
      Some(_) => abort("single-item stream read returned multiple items")
    }
  }
}

///|
fn failure_aware_byte_stream(
  source : @async-core.Stream[
    Result[FixedArray[Byte], @toolStreams.ByteStreamFailure],
  ],
) -> @async-core.Stream[Byte] {
  let state = FailureAwareByteSource::{
    source,
    pending: None,
    offset: 0,
    closed: false,
  }
  @async-core.Stream::from_source(
    count => state.read(count),
    () => {
      if !state.closed {
        state.closed = true
        state.source.drop()
      }
    },
    () => {
      if !state.closed {
        state.closed = true
        state.source.drop_sync()
      }
    },
  )
}

///|
fn byte_stream_items(
  source : @async-core.Stream[Byte],
) -> @async-core.Stream[
  Result[FixedArray[Byte], @toolStreams.ByteStreamFailure],
] {
  @async-core.Stream::from_source(
    count => source.read(count).map(bytes => [Ok(bytes)]),
    () => source.drop(),
    () => source.drop_sync(),
  )
}
