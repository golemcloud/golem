///|
fn run_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  for step = 0; callback != 0; step = step + 1 {
    guard step < 1000 && (callback & 0xf) == 1 else {
      fail("test did not complete without a host wait")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
fn batch(
  payload : Bytes,
  offset : String,
  closed? : Bool = false,
  up_to_date? : Bool = true,
  content_type? : String = "application/json",
) -> @durable-streams.DurableStreamBatch {
  {
    payload: FixedArray::from_array(payload.to_array()),
    content_type,
    next: { offset, cursor: Some("cursor/" + offset), },
    up_to_date,
    closed,
  }
}

///|
fn error(kind : ErrorKind, after? : UInt64) -> HostError {
  {
    kind,
    message: "test failure",
    retry_after_ms: after,
    producer_epoch: None,
    expected_sequence: None,
  }
}

///|
fn receipt(
  sequence : UInt64,
  epoch? : UInt64 = 4,
  closed? : Bool = false,
) -> Receipt {
  { next_offset: Some("opaque:next"), epoch, sequence, closed, }
}

///|
test "reader drains final nested JSON batch before EOF and checkpoint promotion" {
  run_async(async fn() {
    let calls = Ref(0)
    let base : Reader[Array[Int]] = json_reader("https://example.test/stream")
    let reader = {
      ..base,
      read_batch: (request, resource) => {
        calls.val += 1
        assert_eq(request.checkpoint.offset, "-1")
        assert_eq(Some(resource), base.resource)
        Ok(batch(b"[[3,1],[9]]", "opaque:7", closed=true))
      },
    }
    assert_eq(reader.read(), Some([3, 1]))
    assert_eq(reader.checkpoint().offset, "-1")
    assert_eq(calls.val, 1)
    assert_eq(reader.read(), Some([9]))
    assert_eq(reader.checkpoint().offset, "opaque:7")
    assert_eq(reader.checkpoint().cursor, Some("cursor/opaque:7"))
    assert_true(reader.read() is None)
    assert_eq(calls.val, 1)
  })
}

///|
test "now resolves once via catchup then pins content type cursor and SSE" {
  run_async(async fn() {
    let requests : Array[@durable-streams.DurableStreamReadRequest] = []
    let waits : Array[UInt64] = []
    let base : Reader[Int] = json_reader(
      "https://example.test/stream",
      offset="now",
      transport=SSE,
    )
    let reader = {
      ..base,
      read_batch: (request, _) => {
        requests.push(request)
        if requests.length() == 1 {
          Ok(batch(b"[]", "tail:91"))
        } else {
          Ok(batch(b"[17]", "tail:96", closed=true))
        }
      },
      wait: ms => waits.push(ms),
    }
    assert_eq(reader.read(), Some(17))
    assert_eq(requests.length(), 2)
    assert_eq(requests[0].transport, CATCH_UP)
    assert_eq(requests[0].checkpoint.offset, "now")
    assert_eq(requests[1].transport, SSE)
    assert_eq(requests[1].checkpoint.offset, "tail:91")
    assert_eq(requests[1].checkpoint.cursor, Some("cursor/tail:91"))
    assert_eq(requests[1].content_type, Some("application/json"))
    assert_eq(waits, [100])
  })
}

///|
test "byte stream ignores batch boundaries and close prevents later pulls" {
  run_async(async fn() {
    let calls = Ref(0)
    let base = byte_reader("https://example.test/bytes")
    let reader = {
      ..base,
      read_batch: (_, _) => {
        calls.val += 1
        Ok(
          if calls.val == 1 {
            batch(
              b"\x00\xff",
              "A",
              up_to_date=false,
              content_type="application/octet-stream",
            )
          } else {
            batch(b"\x07\x80", "B", content_type="application/octet-stream")
          },
        )
      },
    }
    assert_eq(reader.read(), Some(b'\x00'))
    assert_eq(reader.read(), Some(b'\xff'))
    assert_eq(calls.val, 1)
    assert_eq(reader.read(), Some(b'\x07'))
    assert_eq(calls.val, 2)
    assert_eq(reader.checkpoint().offset, "A")
    reader.close()
    assert_true(reader.read() is None)
    assert_eq(reader.checkpoint().offset, "A")
    assert_eq(calls.val, 2)
  })
}

///|
test "retry kinds retry after and exhausted budget persist across another read" {
  run_async(async fn() {
    let calls = Ref(0)
    let recovered = Ref(false)
    let waits : Array[UInt64] = []
    let base : Reader[Int] = json_reader(
      "https://example.test/stream",
      max_retries=2,
    )
    let reader = {
      ..base,
      read_batch: (_, _) => {
        calls.val += 1
        if recovered.val {
          Ok(batch(b"[23]", "after-outage", closed=true))
        } else {
          Err(error(RATE_LIMITED, after=750))
        }
      },
      wait: ms => waits.push(ms),
    }
    for _ in 0..<2 {
      try reader.read() catch {
        DurableStreamError::Remote(e) => assert_eq(e.kind, RATE_LIMITED)
        e => raise e
      } noraise {
        _ => fail("retry exhaustion must not be EOF")
      }
    }
    assert_eq(calls.val, 4)
    assert_eq(waits, [750, 750])
    assert_eq(reader.retry.failures, 2)
    assert_eq(reader.retry.backoff_ms, 400)
    assert_eq(reader.checkpoint().offset, "-1")
    recovered.val = true
    assert_eq(reader.read(), Some(23))
    assert_eq(calls.val, 5)
    assert_eq(waits, [750, 750])
    assert_eq(reader.retry.failures, 0)
    assert_eq(reader.retry.backoff_ms, 100)
    assert_eq(reader.checkpoint().offset, "after-outage")
    let permanent : Array[ErrorKind] = [
      GONE,
      NOT_FOUND,
      FENCED,
      SEQUENCE_CONFLICT,
      PAYLOAD_TOO_LARGE,
      PERMISSION_DENIED,
      PROTOCOL_ERROR,
    ]
    for kind in permanent {
      let retry = Retry::new(8)
      try retry.delay(error(kind)) catch {
        DurableStreamError::Remote(e) => assert_eq(e.kind, kind)
        e => raise e
      } noraise {
        _ => fail("permanent error was retried")
      }
      retry.allow_explicit_attempt()
      try retry.check() catch {
        DurableStreamError::Remote(e) => assert_eq(e.kind, kind)
        e => raise e
      } noraise {
        _ => fail("explicit attempt cleared permanent error")
      }
    }
    let retry = Retry::new(12)
    let delays = [ for _ in 0..<11 => retry.delay(error(TRANSPORT)) ]
    assert_eq(delays, [
      100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
    ])
  })
}

///|
test "malformed JSON is a sticky failure and item decode retains its index" {
  run_async(async fn() {
    let calls = Ref(0)
    let base : Reader[Int] = json_reader("https://example.test/stream")
    let reader = {
      ..base,
      read_batch: (_, _) => {
        calls.val += 1
        Ok(batch(b"{\"not\":\"array\"}", "bad"))
      },
    }
    for _ in 0..<2 {
      try reader.read() catch {
        _ => ()
      } noraise {
        _ => fail("expected decode failure")
      }
    }
    assert_eq(calls.val, 1)
    assert_eq(reader.checkpoint().offset, "-1")
    let other : Reader[Int] = json_reader("https://example.test/stream")
    other.install(batch(b"[3,\"bad\",9]", "never", closed=true))
    assert_eq(other.read(), Some(3))
    for _ in 0..<2 {
      try other.read() catch {
        _ => ()
      } noraise {
        _ => fail("expected item failure")
      }
    }
    assert_eq(other.pending.unwrap().index, 1)
    assert_eq(other.checkpoint().offset, "-1")
  })
}

///|
test "raw JSON codecs preserve large numeric lexemes and nested arrays" {
  run_async(async fn() {
    let reader : Reader[String] = json_reader_with_codec(
      "https://example.test/stream",
      text => text,
    )
    reader.install(
      batch(
        b"[18446744073709551615,9007199254740993,[1,2]]",
        "end",
        closed=true,
      ),
    )
    assert_eq(reader.read(), Some("18446744073709551615"))
    assert_eq(reader.read(), Some("9007199254740993"))
    assert_eq(reader.read(), Some("[1,2]"))
    let writer = json_writer_with_codec(
      "https://example.test/stream",
      (value : UInt64) => value.to_string(),
      producer_id="p",
      epoch=4,
    )
    writer.prepare([18446744073709551615, 9007199254740993], false)
    assert_eq(
      writer.pending.unwrap().payload,
      @durable-streams.DurableStreamAppendPayload::Json([
        "18446744073709551615", "9007199254740993",
      ]),
    )
    let arrays : Writer[Array[Int]] = json_writer(
      "https://example.test/stream",
      producer_id="array",
    )
    arrays.prepare([[1, 2]], false)
    assert_eq(
      arrays.pending.unwrap().payload,
      @durable-streams.DurableStreamAppendPayload::Json(["[1,2]"]),
    )
  })
}

///|
test "AgentStream source is lazy affine and emits the pending remainder" {
  run_async(async fn() {
    let reader : Reader[Int] = json_reader("https://example.test/stream")
    reader.install(batch(b"[8,3,11]", "end", closed=true))
    assert_eq(reader.read(), Some(8))
    let stream = reader.into_stream()
    try reader.into_stream() catch {
      DurableStreamError::InvalidState(_) => ()
      e => raise e
    } noraise {
      _ => fail("duplicate transfer succeeded")
    }
    try reader.read() catch {
      DurableStreamError::InvalidState(_) => ()
      e => raise e
    } noraise {
      _ => fail("transferred reader remained readable")
    }
    assert_eq(stream.read(), Some(3))
    assert_eq(stream.read(), Some(11))
    assert_true(stream.read() is None)
    assert_eq(reader.checkpoint().offset, "end")
    let unopened = byte_reader("https://example.test/bytes")
    let unused = unopened.into_stream()
    unused.drop()
    assert_true(unopened.closed)
  })
}

///|
test "writer cancellation retains exact body tuple and close before ack" {
  run_async(async fn() {
    let calls : Array[@durable-streams.DurableStreamAppendRequest] = []
    let base = byte_writer(
      "https://example.test/bytes",
      producer_id="stable",
      epoch=4,
    )
    let (gate, promise) : (@async-core.Future[Unit], @async-core.Promise[Unit]) = @async-core.Future::new()
    let (started, started_promise) = @async-core.Future::new()
    let writer = {
      ..base,
      append_batch: async fn(request, _) {
        calls.push(request)
        if calls.length() == 1 {
          ignore(started_promise.complete(()))
          gate.get()
        }
        Ok(receipt(request.sequence, closed=request.close))
      },
    }
    let input : Array[Byte] = [0, 255, 7]
    @async-core.with_task_group(async fn(group) {
      let pending = group.spawn(async fn() { writer.append(input, close=true) })
      started.get()
      input[1] = 3
      assert_true(writer.has_pending())
      pending.cancel()
      try pending.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        e => raise e
      } noraise {
        _ => fail("cancelled append succeeded")
      }
    })
    ignore(promise.close())
    assert_eq(writer.sequence, 0)
    assert_eq(writer.resource, base.resource)
    try writer.append([4]) catch {
      DurableStreamError::InvalidState(_) => ()
      e => raise e
    } noraise {
      _ => fail("uncertain request was overwritten")
    }
    ignore(writer.retry_pending())
    assert_eq(calls.length(), 2)
    assert_eq(calls[0], calls[1])
    assert_eq(
      calls[0].payload,
      @durable-streams.DurableStreamAppendPayload::Bytes([0, 255, 7]),
    )
    assert_true(calls[0].close)
    assert_eq(writer.sequence, 1)
    assert_true(writer.resource is None)
    assert_false(writer.has_pending())
    assert_true(writer.closed)
  })
}

///|
test "writer retry loop keeps exact request and only ack advances sequence" {
  run_async(async fn() {
    let calls : Array[@durable-streams.DurableStreamAppendRequest] = []
    let waits : Array[UInt64] = []
    let base : Writer[Int] = json_writer(
      "https://example.test/stream",
      producer_id="p",
      epoch=4,
    )
    let writer = {
      ..base,
      append_batch: (request, _) => {
        calls.push(request)
        if calls.length() == 1 {
          Err(error(TIMEOUT))
        } else {
          Ok(receipt(request.sequence, closed=request.close))
        }
      },
      wait: ms => waits.push(ms),
    }
    ignore(writer.append([19, 2]))
    assert_eq(calls[0], calls[1])
    assert_eq(waits, [100])
    assert_eq(writer.sequence, 1)
    ignore(writer.close())
    assert_eq(calls[2].sequence, 1)
    assert_eq(
      calls[2].payload,
      @durable-streams.DurableStreamAppendPayload::Json([]),
    )
    assert_true(calls[2].close)
    assert_eq(writer.sequence, 2)
  })
}

///|
test "writer mismatch and sequence exhaustion never clear uncertain state" {
  for ack in [receipt(1), receipt(0, epoch=5), receipt(0, closed=false)] {
    let writer = byte_writer(
      "https://example.test/bytes",
      producer_id="p",
      epoch=4,
    )
    writer.prepare([1], true)
    let pending = writer.pending
    try writer.acknowledge(ack) catch {
      DurableStreamError::Remote(_) => ()
      e => raise e
    } noraise {
      _ => fail("invalid receipt accepted")
    }
    assert_eq(writer.pending, pending)
    assert_eq(writer.sequence, 0)
  }
  let writer = byte_writer(
    "https://example.test/bytes",
    producer_id="p",
    epoch=4,
  )
  try writer.prepare([], false) catch {
    DurableStreamError::InvalidState(_) => ()
    e => raise e
  } noraise {
    _ => fail("empty append accepted")
  }
  writer.sequence = 9007199254740991
  writer.prepare([1], false)
  writer.acknowledge(receipt(9007199254740991))
  try writer.prepare([2], false) catch {
    DurableStreamError::InvalidState(_) => ()
    e => raise e
  } noraise {
    _ => fail("unsafe producer sequence accepted")
  }
}
