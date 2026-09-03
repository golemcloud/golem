///|
test "provider stdout maps every typed failure to the host terminal" {
  assert_eq(
    tool_stream_failure(ToolStreamError::Cancelled),
    @toolHost.ByteStreamFailure::Cancelled,
  )
  assert_eq(
    tool_stream_failure(ToolStreamError::Abandoned),
    @toolHost.ByteStreamFailure::Abandoned,
  )
  assert_eq(
    tool_stream_failure(ToolStreamError::ResourceExhausted),
    @toolHost.ByteStreamFailure::ResourceExhausted,
  )
  assert_eq(
    tool_stream_failure(ToolStreamError::Failed("provider failed")),
    @toolHost.ByteStreamFailure::Failed("provider failed"),
  )
}

///|
fn run_provider_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps = steps + 1
    if steps > 1000 {
      fail("provider stdout async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("provider stdout async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
#warnings("-unused_async")
test "provider stdout writes, finishes, and drops exactly once" {
  run_provider_async(async fn() {
    let writes : Ref[Array[FixedArray[Byte]]] = Ref([])
    let finishes = Ref(0)
    let failures : Ref[Array[@toolHost.ByteStreamFailure]] = Ref([])
    let drops = Ref(0)
    let stdout = provider_stdout_from_callbacks(
      async fn(bytes) {
        writes.val.push(bytes)
        Ok(())
      },
      async fn() {
        finishes.val += 1
        Ok(())
      },
      async fn(reason) {
        failures.val.push(reason)
        Ok(())
      },
      fn() { drops.val += 1 },
    )
    assert_true(stdout.is_open())
    assert_true(stdout.write_all([1, 2, 3]))
    assert_true(stdout.write_all_bytes(b"ok"))
    assert_eq(writes.val, [[1, 2, 3], [111, 107]])
    stdout.finish()
    stdout.finish()
    stdout.fail(ToolStreamError::Cancelled)
    assert_true(!stdout.is_open())
    assert_eq(finishes.val, 1)
    assert_eq(failures.val, [])
    stdout.drop()
    stdout.drop()
    assert_eq(drops.val, 1)
  })
}

///|
#warnings("-unused_async")
test "provider stdout dispatches one typed failure terminal" {
  run_provider_async(async fn() {
    let finishes = Ref(0)
    let failures : Ref[Array[@toolHost.ByteStreamFailure]] = Ref([])
    let drops = Ref(0)
    let stdout = provider_stdout_from_callbacks(
      _ => Ok(()),
      async fn() {
        finishes.val += 1
        Ok(())
      },
      async fn(reason) {
        failures.val.push(reason)
        Ok(())
      },
      fn() { drops.val += 1 },
    )
    stdout.fail(ToolStreamError::Failed("provider failed"))
    stdout.fail(ToolStreamError::Cancelled)
    stdout.finish()
    assert_true(!stdout.is_open())
    assert_eq(failures.val, [
      @toolHost.ByteStreamFailure::Failed("provider failed"),
    ])
    assert_eq(finishes.val, 0)
    stdout.drop()
    assert_eq(drops.val, 1)
  })
}

///|
#warnings("-unused_async")
test "provider stdout closes when the host writer is already closed" {
  run_provider_async(async fn() {
    let drops = Ref(0)
    let stdout = provider_stdout_from_callbacks(
      _ => {
        Err(
          @toolHost.StreamWriteError::Closed(
            @toolHost.ByteStreamCloseCause::ConsumerCancelled,
          ),
        )
      },
      () => Ok(()),
      _ => Ok(()),
      fn() { drops.val += 1 },
    )
    try stdout.write([1]) catch {
      @toolHost.StreamWriteError::Closed(
        @toolHost.ByteStreamCloseCause::ConsumerCancelled
      ) => ()
      _ => fail("unexpected provider stdout write error")
    } noraise {
      _ => fail("closed provider stdout write unexpectedly succeeded")
    }
    assert_true(!stdout.is_open())
    assert_eq(stdout.write([2]), 0)
    stdout.drop()

    let finish_stdout = provider_stdout_from_callbacks(
      _ => Ok(()),
      () => {
        Err(
          @toolHost.StreamWriteError::Closed(
            @toolHost.ByteStreamCloseCause::Finished,
          ),
        )
      },
      _ => Ok(()),
      fn() { drops.val += 1 },
    )
    finish_stdout.finish()
    assert_true(!finish_stdout.is_open())
    finish_stdout.drop()

    let fail_stdout = provider_stdout_from_callbacks(
      _ => Ok(()),
      () => Ok(()),
      _ => {
        Err(
          @toolHost.StreamWriteError::Closed(
            @toolHost.ByteStreamCloseCause::ConsumerCancelled,
          ),
        )
      },
      fn() { drops.val += 1 },
    )
    fail_stdout.fail(ToolStreamError::Abandoned)
    assert_true(!fail_stdout.is_open())
    fail_stdout.drop()
    assert_eq(drops.val, 3)
  })
}

///|
#warnings("-unused_async")
test "provider stdout rejects a concurrent terminal operation" {
  run_provider_async(async fn() {
    let (started, started_promise) = @async-core.Future::new()
    let (blocked, blocked_promise) = @async-core.Future::new()
    let finishes = Ref(0)
    let stdout = provider_stdout_from_callbacks(
      async fn(_) {
        ignore(started_promise.complete(()))
        blocked.get()
        Ok(())
      },
      async fn() {
        finishes.val += 1
        Ok(())
      },
      _ => Ok(()),
      () => (),
    )
    @async-core.with_task_group(async fn(group) {
      let write = group.spawn(async fn() { stdout.write([1]) })
      started.get()
      try stdout.finish() catch {
        @toolHost.StreamWriteError::ConcurrentOperation => ()
        _ => fail("unexpected concurrent provider stdout error")
      } noraise {
        _ => fail("concurrent provider stdout finish unexpectedly succeeded")
      }
      assert_eq(finishes.val, 0)
      ignore(blocked_promise.complete(()))
      assert_eq(write.wait(), 1)
    })
    assert_true(stdout.is_open())
    stdout.finish()
    assert_eq(finishes.val, 1)
    stdout.drop()
  })
}
