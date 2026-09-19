///|
test "provider stdout maps every typed failure to the host terminal" {
  assert_eq(
    tool_stream_failure(@toolCore.ToolStreamError::Cancelled),
    @toolHost.ByteStreamFailure::Cancelled,
  )
  assert_eq(
    tool_stream_failure(@toolCore.ToolStreamError::Abandoned),
    @toolHost.ByteStreamFailure::Abandoned,
  )
  assert_eq(
    tool_stream_failure(@toolCore.ToolStreamError::ResourceExhausted),
    @toolHost.ByteStreamFailure::ResourceExhausted,
  )
  assert_eq(
    tool_stream_failure(@toolCore.ToolStreamError::Failed("provider failed")),
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
    stdout.fail(@toolCore.ToolStreamError::Cancelled)
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
test "provider stdout skips empty writes before calling the host" {
  run_provider_async(async fn() {
    let writes : Ref[Array[FixedArray[Byte]]] = Ref([])
    let stdout = provider_stdout_from_callbacks(
      bytes => {
        writes.val.push(bytes)
        Ok(())
      },
      () => Ok(()),
      _ => Ok(()),
      () => (),
    )
    assert_eq(stdout.write([]), 0)
    assert_eq(stdout.write_bytes(b""), 0)
    assert_eq(writes.val, [])
    assert_eq(stdout.write([42]), 1)
    assert_eq(writes.val, [[42]])
    stdout.finish()
    stdout.drop()
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
    stdout.fail(@toolCore.ToolStreamError::Failed("provider failed"))
    stdout.fail(@toolCore.ToolStreamError::Cancelled)
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
    try finish_stdout.finish() catch {
      @toolHost.StreamWriteError::Closed(
        @toolHost.ByteStreamCloseCause::Finished
      ) => ()
      _ => fail("unexpected finish close cause")
    } noraise {
      _ => fail("finish lost the host close cause")
    }
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
    try fail_stdout.fail(@toolCore.ToolStreamError::Abandoned) catch {
      @toolHost.StreamWriteError::Closed(
        @toolHost.ByteStreamCloseCause::ConsumerCancelled
      ) => ()
      _ => fail("unexpected fail close cause")
    } noraise {
      _ => fail("fail lost the host close cause")
    }
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

///|
test "invocation stdout terminals preserve structured results and original errors" {
  run_provider_async(() => {
    for
      mode in [
        "success", "tool-error", "explicit-failure", "exception", "cancelled", "abandoned",
        "finish-error",
      ] {
      let terminals : Array[String] = []
      let drops = Ref(0)
      let stdout = provider_stdout_from_callbacks(
        _ => Ok(()),
        () => {
          if mode == "finish-error" {
            Err(@toolHost.StreamWriteError::ConcurrentOperation)
          } else {
            terminals.push("finish")
            Ok(())
          }
        },
        reason => {
          terminals.push(
            match reason {
              @toolHost.ByteStreamFailure::Cancelled => "cancelled"
              @toolHost.ByteStreamFailure::ResourceExhausted =>
                "resource-exhausted"
              _ => "failed"
            },
          )
          Ok(())
        },
        () => drops.val += 1,
      )
      let result = try
        invoke_with_stdout(stdout, () => {
          match mode {
            "tool-error" => Err(@types.ToolError::InvalidInput("original"))
            "exception" => raise ToolStreamError::Failed("original")
            "cancelled" => raise @async-core.Cancelled::Cancelled
            _ => {
              if mode == "explicit-failure" {
                stdout.fail(ToolStreamError::ResourceExhausted)
              }
              if mode == "abandoned" {
                stdout.drop()
              }
              Ok(invocation_result_empty())
            }
          }
        })
        |> Ok
      catch {
        error => Err(error)
      }
      match mode {
        "tool-error" => {
          assert_true(
            result is Ok(Err(@types.ToolError::InvalidInput("original"))),
          )
          assert_eq(terminals, ["failed"])
        }
        "exception" => {
          assert_true(result is Err(ToolStreamError::Failed("original")))
          assert_eq(terminals, ["failed"])
        }
        "cancelled" => {
          assert_true(result is Err(@async-core.Cancelled::Cancelled))
          assert_eq(terminals, ["cancelled"])
        }
        "finish-error" => {
          assert_true(
            result is Err(@toolHost.StreamWriteError::ConcurrentOperation),
          )
          assert_eq(terminals, [])
        }
        _ => {
          assert_true(result is Ok(Ok(_)))
          assert_eq(
            terminals,
            if mode == "abandoned" {
              []
            } else if mode == "explicit-failure" {
              ["resource-exhausted"]
            } else {
              ["finish"]
            },
          )
        }
      }
      assert_eq(drops.val, 1)
    }
  })
}

///|
test "invocation completion and drop wait for an outstanding writer operation" {
  run_provider_async(() => {
    for abandon in [false, true] {
      let (started, start) = @async-core.Future::new()
      let (release, unblock) = @async-core.Future::new()
      let (completing, complete_started) = @async-core.Future::new()
      let events : Array[String] = []
      let stdout = provider_stdout_from_callbacks(
        _ => {
          ignore(start.complete(()))
          release.get()
          events.push("write")
          Ok(())
        },
        () => {
          events.push("finish")
          Ok(())
        },
        _ => {
          events.push("fail")
          Ok(())
        },
        () => events.push("drop"),
      )
      @async-core.with_task_group(group => {
        let write = group.spawn(() => stdout.write([7, 2]))
        started.get()
        if abandon {
          stdout.drop()
          stdout.drop()
        }
        assert_eq(events, [])
        let completion = group.spawn(() => {
          invoke_with_stdout(stdout, () => {
            ignore(complete_started.complete(()))
            Ok(invocation_result_empty())
          })
        })
        completing.get()
        assert_eq(events, [])
        ignore(unblock.complete(()))
        let result = completion.wait()
        assert_true(result is Ok(_))
        assert_eq(write.wait(), 2)
      })
      assert_eq(
        events,
        if abandon {
          ["write", "drop"]
        } else {
          ["write", "finish", "drop"]
        },
      )
    }
  })
}

///|
test "completion drains every chunk of an already started write all" {
  run_provider_async(() => {
    let payload = Array::makei(193, i => (i % 251).to_byte())
    let actual : Array[Byte] = []
    let (started, start) = @async-core.Future::new()
    let (release, unblock) = @async-core.Future::new()
    let (completing, complete_started) = @async-core.Future::new()
    let stdout = provider_stdout_from_callbacks(
      bytes => {
        if actual.is_empty() {
          ignore(start.complete(()))
          release.get()
        }
        for byte in bytes {
          actual.push(byte)
        }
        Ok(())
      },
      () => {
        assert_eq(actual, payload)
        Ok(())
      },
      _ => Ok(()),
      () => (),
    )
    @async-core.with_task_group(group => {
      let write = group.spawn(() => stdout.write_all(payload))
      started.get()
      let completion = group.spawn(() => {
        invoke_with_stdout(stdout, () => {
          ignore(complete_started.complete(()))
          Ok(invocation_result_empty())
        })
      })
      completing.get()
      ignore(unblock.complete(()))
      assert_true(write.wait())
      assert_true(completion.wait() is Ok(_))
    })
    assert_eq(actual, payload)
  })
}

///|
test "real task cancellation during invocation and completion releases stdout and discarded result" {
  run_provider_async(() => {
    for during_finish in [false, true] {
      let (started, start) = @async-core.Future::new()
      let (release, unblock) = @async-core.Future::new()
      let (fail_started, start_failure) = @async-core.Future::new()
      let (release_failure, unblock_failure) = @async-core.Future::new()
      let drops = Ref(0)
      let result_drops = Ref(0)
      let terminals : Array[String] = []
      let stdout = provider_stdout_from_callbacks(
        _ => Ok(()),
        () => {
          ignore(start.complete(()))
          release.get()
          terminals.push("finish")
          Ok(())
        },
        reason => {
          assert_true(reason is @toolHost.ByteStreamFailure::Cancelled)
          ignore(start_failure.complete(()))
          release_failure.get()
          terminals.push("cancelled")
          Ok(())
        },
        () => drops.val += 1,
      )
      @async-core.with_task_group(group => {
        let task = group.spawn(() => {
          invoke_with_stdout(stdout, () => {
            if !during_finish {
              ignore(start.complete(()))
              release.get()
            }
            let result_stream = @async-core.Stream::from_source(
              _ => None,
              () => (),
              () => result_drops.val += 1,
            )
            Ok({ result: None, stdout: Some(result_stream), })
          })
        })
        started.get()
        task.cancel()
        if during_finish {
          let (turn, next_turn) = @async-core.Future::new()
          group.spawn_bg(() => ignore(next_turn.complete(())))
          turn.get()
          assert_true(task.try_wait() is None)
          assert_eq(drops.val, 0)
          assert_eq(terminals, [])
          ignore(unblock.complete(()))
        } else {
          fail_started.get()
          assert_true(task.try_wait() is None)
          assert_eq(drops.val, 0)
          assert_eq(terminals, [])
          ignore(unblock_failure.complete(()))
        }
        try task.wait() catch {
          @async-core.Cancelled::Cancelled => ()
          _ => fail("expected task cancellation")
        } noraise {
          _ => fail("task cancellation was lost")
        }
      })
      assert_eq(drops.val, 1)
      assert_eq(result_drops.val, if during_finish { 1 } else { 0 })
      assert_eq(
        terminals,
        if during_finish {
          ["finish"]
        } else {
          ["cancelled"]
        },
      )
    }
  })
}
