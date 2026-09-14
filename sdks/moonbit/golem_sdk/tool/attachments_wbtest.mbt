///|
fn run_attachment_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps = steps + 1
    if steps > 1000 {
      fail("attachment async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("attachment async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
fn assert_encoded_source_failure(
  error : ToolStreamError,
  expected : @toolHost.ByteStreamFailure,
) -> Unit raise {
  run_attachment_async(async fn() {
    let closes = Ref(0)
    let closed = Ref(false)
    let close = fn() {
      if !closed.val {
        closed.val = true
        closes.val += 1
      }
    }
    let source = @async-core.Stream::from_source(
      _ => raise error,
      fn() { close() },
      close,
    )
    let encoded = stdin_byte_stream(source)
    guard encoded.read(1) is Some(items) && items.length() == 1 else {
      fail("expected one encoded stdin failure")
    }
    guard items[0] is Err(actual) else {
      fail("expected an encoded stdin failure")
    }
    assert_eq(actual, expected)
    encoded.drop_sync()
    assert_eq(closes.val, 1)
  })
}

///|
test "stdin preserves known tool stream failure variants" {
  assert_encoded_source_failure(
    ToolStreamError::Cancelled,
    @toolHost.ByteStreamFailure::Cancelled,
  )
  assert_encoded_source_failure(
    ToolStreamError::Abandoned,
    @toolHost.ByteStreamFailure::Abandoned,
  )
  assert_encoded_source_failure(
    ToolStreamError::ResourceExhausted,
    @toolHost.ByteStreamFailure::ResourceExhausted,
  )
  assert_encoded_source_failure(
    ToolStreamError::Failed("source failed"),
    @toolHost.ByteStreamFailure::Failed("source failed"),
  )
}

///|
test "stdin skips empty source chunks before producing host items" {
  run_attachment_async(async fn() {
    let mut reads = 0
    let source : @async-core.Stream[Byte] = @async-core.Stream::from_source(
      fn(_) {
        reads += 1
        match reads {
          1 => Some([])
          2 => Some([42])
          _ => None
        }
      },
      () => (),
      () => (),
    )
    let encoded = stdin_byte_stream(source)
    guard encoded.read(1) is Some(items) && items.length() == 1 else {
      fail("expected one non-empty encoded stdin item")
    }
    assert_eq(items[0], Ok([42]))
    assert_eq(encoded.read(1), None)
  })
}

///|
test "stdin preserves task cancellation as stream cancellation" {
  run_attachment_async(async fn() {
    let closes = Ref(0)
    let source = @async-core.Stream::from_source(
      _ => raise @async-core.Cancelled::Cancelled,
      fn() { closes.val += 1 },
      fn() { closes.val += 1 },
    )
    let encoded = stdin_byte_stream(source)
    guard encoded.read(1) is Some(items) && items.length() == 1 else {
      fail("expected one encoded stdin cancellation")
    }
    assert_true(items[0] is Err(@toolHost.ByteStreamFailure::Cancelled))
    encoded.drop_sync()
    assert_eq(closes.val, 1)
  })
}

///|
test "cancelling a stdin producer publishes cancellation and closes its source" {
  run_attachment_async(async fn() {
    let (started, started_promise) = @async-core.Future::new()
    let (blocked, _blocked_promise) = @async-core.Future::new()
    let closes = Ref(0)
    let closed = Ref(false)
    let close = fn() {
      if !closed.val {
        closed.val = true
        closes.val += 1
      }
    }
    let source = @async-core.Stream::from_source(
      async fn(_) {
        ignore(started_promise.complete(()))
        blocked.get()
        None
      },
      fn() { close() },
      close,
    )
    let producer = match stdin_byte_stream(source).take_producer() {
      Some(producer) => producer
      None => abort("stdin stream did not expose its producer")
    }
    let (encoded, sink) = @async-core.Stream::new(capacity=1)
    @async-core.with_task_group(async fn(group) {
      let task = group.spawn(async fn() { producer(sink) })
      started.get()
      task.cancel()
      task.wait()
    })
    guard encoded.read(1) is Some(items) && items.length() == 1 else {
      fail("cancelled stdin producer did not publish one terminal")
    }
    assert_true(items[0] is Err(@toolHost.ByteStreamFailure::Cancelled))
    assert_eq(closes.val, 1)
    encoded.drop()
  })
}

///|
priv suberror UnknownSourceFailure derive(Debug)

///|
test "stdin maps unknown source errors to generic failure" {
  run_attachment_async(async fn() {
    let closes = Ref(0)
    let source = @async-core.Stream::from_source(
      _ => raise UnknownSourceFailure,
      fn() { closes.val += 1 },
      fn() { closes.val += 1 },
    )
    let encoded = stdin_byte_stream(source)
    guard encoded.read(1) is Some(items) && items.length() == 1 else {
      fail("expected one encoded stdin failure")
    }
    guard items[0] is Err(@toolHost.ByteStreamFailure::Failed(message)) else {
      fail("expected a generic encoded stdin failure")
    }
    assert_true(message.contains("UnknownSourceFailure"))
    encoded.drop_sync()
    assert_eq(closes.val, 1)
  })
}

///|
test "host stream failures retain their SDK identity" {
  assert_eq(
    stream_failure(@toolHost.ByteStreamFailure::Cancelled),
    ToolStreamError::Cancelled,
  )
  assert_eq(
    stream_failure(@toolHost.ByteStreamFailure::Abandoned),
    ToolStreamError::Abandoned,
  )
  assert_eq(
    stream_failure(@toolHost.ByteStreamFailure::ResourceExhausted),
    ToolStreamError::ResourceExhausted,
  )
  assert_eq(
    stream_failure(@toolHost.ByteStreamFailure::Failed("host failed")),
    ToolStreamError::Failed("host failed"),
  )
}

///|
test "byte stream surfaces a host terminal failure" {
  run_attachment_async(async fn() {
    let mut read = false
    let source : @async-core.Stream[
      Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
    ] = @async-core.Stream::from_source(
      fn(_) {
        if read {
          None
        } else {
          read = true
          Some([Err(@toolHost.ByteStreamFailure::ResourceExhausted)])
        }
      },
      () => (),
      () => (),
    )
    let bytes = byte_stream(source)
    let terminal = try bytes.read(1) catch {
      ToolStreamError::ResourceExhausted => "resource-exhausted"
      error => "unexpected: \{Repr(error)}"
    } noraise {
      None => "eof"
      Some(_) => "data"
    }
    assert_eq(terminal, "resource-exhausted")
    assert_eq(bytes.read(1), None)
  })
}

///|
test "byte stream retains unread bytes between bounded reads" {
  run_attachment_async(async fn() {
    let mut read = false
    let source : @async-core.Stream[
      Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
    ] = @async-core.Stream::from_source(
      fn(_) {
        if read {
          None
        } else {
          read = true
          Some([Ok([1, 2, 3, 4])])
        }
      },
      () => (),
      () => (),
    )
    let bytes = byte_stream(source)
    assert_eq(bytes.read(2), Some([1, 2]))
    assert_eq(bytes.read(1), Some([3]))
    assert_eq(bytes.read(2), Some([4]))
    assert_eq(bytes.read(1), None)
  })
}

///|
test "byte stream skips empty successful host items" {
  run_attachment_async(async fn() {
    let mut reads = 0
    let source : @async-core.Stream[
      Result[FixedArray[Byte], @toolHost.ByteStreamFailure],
    ] = @async-core.Stream::from_source(
      fn(_) {
        reads += 1
        match reads {
          1 => Some([Ok([])])
          2 => Some([Ok([42])])
          _ => None
        }
      },
      () => (),
      () => (),
    )
    let bytes = byte_stream(source)
    assert_eq(bytes.read(1), Some([42]))
    assert_eq(bytes.read(1), None)
  })
}
