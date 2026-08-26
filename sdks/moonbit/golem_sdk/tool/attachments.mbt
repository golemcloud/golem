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
