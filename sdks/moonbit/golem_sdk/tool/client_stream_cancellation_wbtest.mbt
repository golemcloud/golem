///|
fn run_tool_client_stream_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    if steps > 1000 {
      fail("tool client stream async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("tool client stream async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
extern "wasm" fn reset_tool_client_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn tool_client_resource_drop_count(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
extern "wasm" fn set_tool_client_stream_host_mode(mode : Int) = "golem:test" "set-schema-value-stream-host-mode"

///|
test "client input encoding preserves cancellation and drops input and stdin" {
  reset_tool_client_resource_drop_counts()
  let input_producer_started = [false]
  let input_cleanup_runs = [0]
  let stdin_started = [false]
  let stdin_cleanup_runs = [0]
  run_tool_client_stream_async(async fn() {
    set_tool_client_stream_host_mode(3)
    defer set_tool_client_stream_host_mode(0)
    let captured = @types.Secret::Secret(150)
    let input_stream : @schema.AgentStream[Int] = @schema.AgentStream::produce(
      async fn(writer) {
        input_producer_started[0] = true
        captured.drop()
        writer.close()
      },
      on_unstarted_drop=fn() {
        input_cleanup_runs[0] += 1
        captured.drop()
      },
    )
    let input = @schema.try_into_typed_schema_value(input_stream)
    let stdin : @async-core.Stream[Byte] = @async-core.Stream::produce(
      async fn(sink) {
        stdin_started[0] = true
        sink.close()
      },
      on_unstarted_drop=fn() { stdin_cleanup_runs[0] += 1 },
    )
    let rpc = @toolHost.ToolRpc::ToolRpc(151)
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let invoke = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        let result : Result[InvocationResult, ToolError[String]] = invoke_and_await(
          rpc,
          [],
          input,
          Some(stdin),
          fn(_) { Err("unused tool error decoder") },
        )
        result
      })
      started.get()
      invoke.cancel()
      try invoke.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error => fail("tool client changed cancellation: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled tool client invocation unexpectedly completed")
      }
    })
  })
  assert_false(input_producer_started[0])
  assert_eq(input_cleanup_runs[0], 1)
  assert_false(stdin_started[0])
  assert_eq(stdin_cleanup_runs[0], 1)
  assert_eq(tool_client_resource_drop_count(0), 1)
}
