///|
fn run_tool_core_stream_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    if steps > 1000 {
      fail("tool-core stream async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("tool-core stream async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
extern "wasm" fn reset_tool_core_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn tool_core_resource_drop_count(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
extern "wasm" fn set_tool_core_stream_host_mode(mode : Int) = "golem:test" "set-schema-value-stream-host-mode"

///|
fn cancellable_tool_result_stream(
  raw_secret : Int,
  producer_started : Array[Bool],
  cleanup_runs : Array[Int],
) -> @schema.AgentStream[Int] {
  let captured = @types.Secret::Secret(raw_secret)
  @schema.AgentStream::produce(
    async fn(writer) {
      producer_started[0] = true
      captured.drop()
      writer.close()
    },
    on_unstarted_drop=fn() {
      cleanup_runs[0] += 1
      captured.drop()
    },
  )
}

///|
test "result encoding preserves cancellation and drops result and stdout" {
  reset_tool_core_resource_drop_counts()
  let producer_started = [false]
  let result_cleanup_runs = [0]
  let stdout_started = [false]
  let stdout_cleanup_runs = [0]
  run_tool_core_stream_async(async fn() {
    set_tool_core_stream_host_mode(3)
    defer set_tool_core_stream_host_mode(0)
    let result = cancellable_tool_result_stream(
      140, producer_started, result_cleanup_runs,
    )
    let stdout : @async-core.Stream[Byte] = @async-core.Stream::produce(
      async fn(sink) {
        stdout_started[0] = true
        sink.close()
      },
      on_unstarted_drop=fn() { stdout_cleanup_runs[0] += 1 },
    )
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let encode = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        invocation_result_value_with_stdout(result, stdout)
      })
      started.get()
      encode.cancel()
      try encode.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error =>
          fail("tool result encoding changed cancellation: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled tool result encoding unexpectedly completed")
      }
    })
  })
  assert_false(producer_started[0])
  assert_eq(result_cleanup_runs[0], 1)
  assert_false(stdout_started[0])
  assert_eq(stdout_cleanup_runs[0], 1)
  assert_eq(tool_core_resource_drop_count(0), 1)
}

///|
test "custom error encoding preserves cancellation and drops its payload" {
  reset_tool_core_resource_drop_counts()
  let producer_started = [false]
  let cleanup_runs = [0]
  run_tool_core_stream_async(async fn() {
    set_tool_core_stream_host_mode(3)
    defer set_tool_core_stream_host_mode(0)
    let payload = @schema.try_into_typed_schema_value(
      cancellable_tool_result_stream(141, producer_started, cleanup_runs),
    )
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let encode = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        encode_error_payload(payload)
      })
      started.get()
      encode.cancel()
      try encode.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error =>
          fail("tool error encoding changed cancellation: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled tool error encoding unexpectedly completed")
      }
    })
  })
  assert_false(producer_started[0])
  assert_eq(cleanup_runs[0], 1)
  assert_eq(tool_core_resource_drop_count(0), 1)
}
