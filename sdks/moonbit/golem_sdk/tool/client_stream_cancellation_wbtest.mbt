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
extern "wasm" fn set_tool_client_host_mode(mode : Int) = "golem:test" "set-tool-host-mode"

///|
fn empty_tool_input() -> @types.TypedSchemaValue raise {
  let value = @schema.try_into_typed_schema_value(())
  @model.typed_schema_value_to_wit(value)
}

///|
fn start_output_invocation(
  client : ToolClient,
) -> ToolInvocation[NoToolError] raise {
  match
    client.start(["stream"], empty_tool_input(), None, true, false, fn(_, _) {
      None
    }) {
    Ok(invocation) => invocation
    Err(_) => fail("failed to start output invocation")
  }
}

///|
priv suberror UnknownCollectedFailure derive(Debug)

///|
test "collection maps unknown channel errors without hiding sibling outcomes" {
  run_tool_client_stream_async(async fn() {
    let failed = @async-core.Stream::from_source(
      _ => raise UnknownCollectedFailure,
      () => (),
      () => (),
    )
    let mut emitted = false
    let stderr : @async-core.Stream[Byte] = @async-core.Stream::from_source(
      _ => {
        if emitted {
          None
        } else {
          emitted = true
          Some([b'\x05', b'\x06'])
        }
      },
      () => (),
      () => (),
    )
    let collected : CollectedToolInvocation[String, NoToolError] = collect_tool_channels(
      Some(failed),
      Some(stderr),
      async fn() { raise UnknownCollectedFailure },
    )
    guard collected.stdout is Err(ToolStreamError::Failed(stdout_message)) else {
      fail("unknown stdout error was not collected")
    }
    guard collected.result
      is Err(ToolError::Rpc(RpcError::Protocol(result_message))) else {
      fail("unknown result error was not collected")
    }
    assert_true(stdout_message.contains("UnknownCollectedFailure"))
    assert_true(result_message.contains("UnknownCollectedFailure"))
    assert_eq(collected.stderr, Ok(Some(b"\x05\x06")))
  })
}

///|
test "collection preserves task cancellation" {
  run_tool_client_stream_async(async fn() {
    let stream = @async-core.Stream::from_source(
      _ => raise @async-core.Cancelled::Cancelled,
      () => (),
      () => (),
    )
    let result : async () -> Result[String, ToolError[NoToolError]] = async fn() {
      Ok("unreachable")
    }
    try collect_tool_channels(Some(stream), None, result) catch {
      @async-core.Cancelled::Cancelled => ()
      error => fail("stream collection changed cancellation: \{repr(error)}")
    } noraise {
      _ => fail("stream cancellation became a collected outcome")
    }
    let cancelled_result : async () -> Result[String, ToolError[NoToolError]] = async fn() {
      raise @async-core.Cancelled::Cancelled
    }
    try collect_tool_channels(None, None, cancelled_result) catch {
      @async-core.Cancelled::Cancelled => ()
      error => fail("result collection changed cancellation: \{repr(error)}")
    } noraise {
      _ => fail("result cancellation became a collected outcome")
    }
  })
}

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
          fn(_, _) { Some(Err("unused tool error decoder")) },
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

///|
test "explicit invocation cancellation is distinct from observer and output drop" {
  reset_tool_client_resource_drop_counts()
  set_tool_client_host_mode(1)
  defer set_tool_client_host_mode(0)
  run_tool_client_stream_async(async fn() {
    let created : Result[ToolClient, ToolError[NoToolError]] = ToolClient::try_new(
      "terminal-test",
    )
    let client = match created {
      Ok(client) => client
      Err(_) => fail("failed to create tool client")
    }
    defer client.drop()
    let invocation = start_output_invocation(client)
    invocation.cancel()
    assert_eq(tool_client_resource_drop_count(5), 1)
    assert_eq(tool_client_resource_drop_count(4), 0)
    assert_eq(tool_client_resource_drop_count(6), 0)
    match invocation.get() {
      Err(ToolError::Rpc(RpcError::Cancelled)) => ()
      _ => fail("explicit cancellation did not select cancelled result")
    }
    invocation.drop()
  })
  assert_eq(tool_client_resource_drop_count(4), 1)
  assert_eq(tool_client_resource_drop_count(6), 1)
}

///|
test "dropping a raw result observer does not cancel its output reader" {
  reset_tool_client_resource_drop_counts()
  set_tool_client_host_mode(2)
  defer set_tool_client_host_mode(0)
  run_tool_client_stream_async(async fn() {
    let rpc = match @toolHost.ToolRpc::create("terminal-test") {
      Ok(rpc) => rpc
      Err(_) => fail("failed to create raw tool client")
    }
    defer rpc.drop()
    let (target, output) = @toolHost.create_output()
    let observer = rpc.async_invoke_and_await(
      ["stream"],
      empty_tool_input(),
      None,
      Some(target),
      None,
    )
    observer.drop()
    assert_eq(tool_client_resource_drop_count(4), 1)
    assert_eq(tool_client_resource_drop_count(5), 0)
    assert_eq(tool_client_resource_drop_count(6), 0)
    output.drop()
    assert_eq(tool_client_resource_drop_count(6), 1)
  })
}

///|
test "detaching a generated invocation result preserves its output reader" {
  reset_tool_client_resource_drop_counts()
  set_tool_client_host_mode(2)
  defer set_tool_client_host_mode(0)
  run_tool_client_stream_async(async fn() {
    let created : Result[ToolClient, ToolError[NoToolError]] = ToolClient::try_new(
      "terminal-test",
    )
    let client = match created {
      Ok(client) => client
      Err(_) => fail("failed to create tool client")
    }
    defer client.drop()
    let invocation = start_output_invocation(client)
    invocation.detach_result_observer()
    assert_eq(tool_client_resource_drop_count(4), 1)
    assert_eq(tool_client_resource_drop_count(5), 0)
    assert_eq(tool_client_resource_drop_count(6), 0)
    match invocation.get() {
      Err(ToolError::Rpc(RpcError::Protocol("result observer was detached"))) =>
        ()
      _ => fail("detached invocation result remained observable")
    }
    invocation.cancel()
    assert_eq(tool_client_resource_drop_count(5), 0)
    invocation.drop()
  })
  assert_eq(tool_client_resource_drop_count(4), 1)
  assert_eq(tool_client_resource_drop_count(6), 1)
}

///|
test "cancelling a blocked output read preserves the result observer" {
  reset_tool_client_resource_drop_counts()
  set_tool_client_host_mode(3)
  defer set_tool_client_host_mode(0)
  run_tool_client_stream_async(async fn() {
    let created : Result[ToolClient, ToolError[NoToolError]] = ToolClient::try_new(
      "terminal-test",
    )
    let client = match created {
      Ok(client) => client
      Err(_) => fail("failed to create tool client")
    }
    defer client.drop()
    let invocation = start_output_invocation(client)
    let stdout = invocation.stdout.unwrap()
    @async-core.with_task_group(async fn(group) {
      let read = group.spawn(async fn() { stdout.read(1) })
      let (turn, next_turn) = @async-core.Future::new()
      group.spawn_bg(fn() { ignore(next_turn.complete(())) })
      turn.get()
      assert_true(read.try_wait() is None)
      read.cancel()
      try read.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        _ => fail("blocked output read raised the wrong cancellation")
      } noraise {
        _ => fail("blocked output read unexpectedly completed")
      }
    })
    assert_eq(tool_client_resource_drop_count(7), 1)
    assert_eq(tool_client_resource_drop_count(6), 1)
    assert_eq(tool_client_resource_drop_count(5), 0)
    match invocation.get() {
      Ok(_) => ()
      Err(_) => fail("reader cancellation changed the invocation result")
    }
    invocation.drop()
  })
  assert_eq(tool_client_resource_drop_count(4), 1)
}
