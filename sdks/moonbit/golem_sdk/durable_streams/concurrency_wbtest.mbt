///|
test "one writer serializes concurrent appends without reusing the sequence" {
  run_async(async fn() {
    let calls : Array[@durable-streams.DurableStreamAppendRequest] = []
    let (gate, release) : (@async-core.Future[Unit], @async-core.Promise[Unit]) = @async-core.Future::new()
    let (started, signal) = @async-core.Future::new()
    let (queued, signal_queued) = @async-core.Future::new()
    let base : Writer[Int] = json_writer(
      "https://example.test/stream",
      producer_id="p",
      epoch=4,
    )
    let writer = {
      ..base,
      append_batch: async fn(request, _) {
        calls.push(request)
        if calls.length() == 1 {
          ignore(signal.complete(()))
          gate.get()
        }
        Ok(receipt(request.sequence))
      },
    }
    @async-core.with_task_group(async fn(group) {
      let first = group.spawn(async fn() { writer.append([11]) })
      started.get()
      let second = group.spawn(async fn() {
        ignore(signal_queued.complete(()))
        writer.append([3])
      })
      queued.get()
      assert_eq(calls.length(), 1)
      assert_eq(writer.sequence, 0)
      try writer.drop() catch {
        DurableStreamError::InvalidState(_) => ()
        e => raise e
      } noraise {
        _ => fail("dropped resource borrowed by active append")
      }
      ignore(release.complete(()))
      ignore(first.wait())
      ignore(second.wait())
    })
    assert_eq(calls.length(), 2)
    assert_eq(calls[0].sequence, 0)
    assert_eq(calls[1].sequence, 1)
    assert_eq(
      calls[0].payload,
      @durable-streams.DurableStreamAppendPayload::Json(["11"]),
    )
    assert_eq(
      calls[1].payload,
      @durable-streams.DurableStreamAppendPayload::Json(["3"]),
    )
  })
}

///|
test "independent writers can finish in either order" {
  for reverse in [false, true] {
    run_async(async fn() {
      let (gate1, release1) : (
        @async-core.Future[Unit],
        @async-core.Promise[Unit],
      ) = @async-core.Future::new()
      let (gate2, release2) : (
        @async-core.Future[Unit],
        @async-core.Promise[Unit],
      ) = @async-core.Future::new()
      let (started1, signal1) = @async-core.Future::new()
      let (started2, signal2) = @async-core.Future::new()
      let base1 = byte_writer(
        "https://example.test/one",
        producer_id="one",
        epoch=4,
      )
      let base2 = byte_writer(
        "https://example.test/two",
        producer_id="two",
        epoch=4,
      )
      let writer1 = {
        ..base1,
        append_batch: async fn(request, _) {
          ignore(signal1.complete(()))
          gate1.get()
          Ok(receipt(request.sequence))
        },
      }
      let writer2 = {
        ..base2,
        append_batch: async fn(request, _) {
          ignore(signal2.complete(()))
          gate2.get()
          Ok(receipt(request.sequence))
        },
      }
      @async-core.with_task_group(async fn(group) {
        let first = group.spawn(async fn() { writer1.append([1]) })
        let second = group.spawn(async fn() { writer2.append([2]) })
        started1.get()
        started2.get()
        if reverse {
          ignore(release2.complete(()))
          ignore(second.wait())
          assert_true(writer1.has_pending())
          ignore(release1.complete(()))
          ignore(first.wait())
        } else {
          ignore(release1.complete(()))
          ignore(first.wait())
          assert_true(writer2.has_pending())
          ignore(release2.complete(()))
          ignore(second.wait())
        }
      })
      assert_eq(writer1.sequence, 1)
      assert_eq(writer2.sequence, 1)
      assert_true(writer1.resource != writer2.resource)
    })
  }
}

///|
test "native producer failure fails its operation task group" {
  run_async(async fn() {
    let base : Reader[Int] = json_reader("https://example.test/stream")
    let reader = { ..base, read_batch: (_, _) => Err(error(GONE)), }
    let @durable-streams.DurableStreamReader(handle) = reader.resource.unwrap()
    try
      @async-core.with_task_group(async fn(_) {
        let stream = reader.into_stream()
        ignore(stream.read())
      })
    catch {
      DurableStreamError::Remote(e) => assert_eq(e.kind, GONE)
      e => raise e
    } noraise {
      _ => fail("producer failure disappeared at operation boundary")
    }
    assert_eq(resource_stat(handle, 3), 1)
  })
}

///|
test "nested schema transfer preserves the ordinary native stream source" {
  run_async(async fn() {
    let reader : Reader[Int] = json_reader("https://example.test/stream")
    reader.install(batch(b"[13,4]", "end", closed=true))
    let stream = reader.into_stream()
    let value = @schema.to_value_as((Some(stream), "metadata"))
    let (restored, metadata) : (@schema.AgentStream[Int]?, String) = @schema.from_value_as(
      value,
    )
    assert_eq(metadata, "metadata")
    let restored = restored.unwrap()
    assert_eq(restored.read(), Some(13))
    assert_eq(restored.read(), Some(4))
    assert_true(restored.read() is None)
  })
}

///|
test "closing during a host read discards completion without advancing" {
  run_async(async fn() {
    let (gate, release) : (@async-core.Future[Unit], @async-core.Promise[Unit]) = @async-core.Future::new()
    let (started, signal) = @async-core.Future::new()
    let base = byte_reader("https://example.test/bytes")
    let reader = {
      ..base,
      read_batch: async fn(_, _) {
        ignore(signal.complete(()))
        gate.get()
        Ok(batch(b"abc", "unconsumed"))
      },
    }
    @async-core.with_task_group(async fn(group) {
      let pull = group.spawn(async fn() { reader.read() })
      started.get()
      reader.close()
      assert_true(reader.resource is Some(_))
      ignore(release.complete(()))
      assert_true(pull.wait() is None)
    })
    assert_true(reader.pending is None)
    assert_true(reader.resource is None)
    assert_eq(reader.checkpoint().offset, "-1")
  })
}

///|
test "cancelled idle wait retains resolved now and retry_pending retains exhausted budget" {
  run_async(async fn() {
    let requests : Array[@durable-streams.DurableStreamReadRequest] = []
    let base : Reader[Int] = json_reader(
      "https://example.test/stream",
      offset="now",
      transport=SSE,
    )
    let reader = {
      ..base,
      read_batch: (request, _) => {
        requests.push(request)
        Ok(
          if requests.length() == 1 {
            batch(b"[]", "fixed-tail")
          } else {
            batch(b"[6]", "end", closed=true)
          },
        )
      },
      wait: _ => raise @async-core.Cancelled::Cancelled,
    }
    try reader.read() catch {
      @async-core.Cancelled::Cancelled => ()
      e => raise e
    } noraise {
      _ => fail("idle wait was not cancelled")
    }
    assert_eq(reader.checkpoint().offset, "fixed-tail")
    assert_eq(reader.read(), Some(6))
    assert_eq(requests[1].checkpoint.offset, "fixed-tail")
    assert_eq(requests[1].transport, SSE)
    let calls : Array[@durable-streams.DurableStreamAppendRequest] = []
    let recovered = Ref(false)
    let waits : Array[UInt64] = []
    let writer_base : Writer[Int] = json_writer(
      "https://example.test/stream",
      producer_id="p",
      max_retries=2,
    )
    let writer = {
      ..writer_base,
      append_batch: (request, _) => {
        calls.push(request)
        if recovered.val {
          Ok({
            next_offset: None,
            epoch: writer_base.epoch,
            sequence: request.sequence,
            closed: request.close,
          })
        } else {
          Err(error(UNAVAILABLE))
        }
      },
      wait: ms => waits.push(ms),
    }
    try writer.append([17]) catch {
      DurableStreamError::Remote(e) => assert_eq(e.kind, UNAVAILABLE)
      e => raise e
    } noraise {
      _ => fail("failed append acknowledged")
    }
    let pending = writer.pending
    try writer.retry_pending() catch {
      DurableStreamError::Remote(e) => assert_eq(e.kind, UNAVAILABLE)
      e => raise e
    } noraise {
      _ => fail("exhausted retry budget reset")
    }
    assert_eq(calls.length(), 4)
    assert_eq(writer.pending, pending)
    assert_eq(writer.retry.failures, 2)
    assert_eq(writer.retry.backoff_ms, 400)
    assert_eq(waits, [100, 200])
    recovered.val = true
    assert_eq(writer.retry_pending().next_offset, None)
    assert_eq(calls.length(), 5)
    for call in calls {
      assert_eq(call, pending.unwrap())
    }
    assert_eq(
      calls[4].payload,
      @durable-streams.DurableStreamAppendPayload::Json(["17"]),
    )
    assert_eq(waits, [100, 200])
    assert_eq(writer.retry.failures, 0)
    assert_eq(writer.retry.backoff_ms, 100)
    assert_false(writer.has_pending())
    assert_eq(writer.sequence, 1)
  })
}
