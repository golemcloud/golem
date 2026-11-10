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
    if (callback & 0xf) == 1 {
      callback = @async-core.cb(0, 0, 0)
    } else {
      callback = @async-core.cb(1, settle_tool_client_result(), 2)
    }
  }
}

///|
extern "wasm" fn reset_tool_client_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn tool_client_resource_drop_count(kind : Int32) -> Int32 = "golem:test" "resource-drop-count"

///|
extern "wasm" fn set_tool_client_stream_host_mode(mode : Int32) = "golem:test" "set-schema-value-stream-host-mode"

///|
extern "wasm" fn set_tool_client_host_mode(mode : Int32) = "golem:test" "set-tool-host-mode"

///|
extern "wasm" fn settle_tool_client_result() -> Int32 = "golem:test" "settle-tool-result"

///|
#owned(bytes)
extern "wasm" fn tool_client_bytes_pointer(bytes : Bytes) -> Int32 =
  #|(func (param i32) (result i32) local.get 0)

///|
extern "wasm" fn set_tool_client_result_node(
  pointer : Int32,
  types : Int32,
  defs : Int32,
  name : Int32,
) = "golem:test" "tool-result-node"

///|
fn tool_client_reply_secret() -> Unit {
  let node = Array::make(32, b'\x00')
  node[0] = b'\x20'
  node[8] = b'\x2a'
  set_tool_client_result_node(
    tool_client_bytes_pointer(Bytes::from_array(node)),
    tool_client_bytes_pointer(b""),
    tool_client_bytes_pointer(b""),
    tool_client_bytes_pointer(b""),
  )
}

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
    let (started, start) = @async-core.Future::new()
    let (blocked, _unblock) = @async-core.Future::new()
    let stream = @async-core.Stream::from_source(
      async fn(_) {
        ignore(start.complete(()))
        blocked.get()
        None
      },
      () => (),
      () => (),
    )
    let result : async () -> Result[String, ToolError[NoToolError]] = async fn() {
      Ok("unreachable")
    }
    @async-core.with_task_group(group => {
      let task = group.spawn(() => {
        collect_tool_channels(Some(stream), None, result)
      })
      started.get()
      task.cancel()
      try task.wait() catch {
        @async-core.TaskCancelled::TaskCancelled => ()
        error => fail("stream collection changed cancellation: \{repr(error)}")
      } noraise {
        _ => fail("stream cancellation became a collected outcome")
      }
    })
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
    let input_stream : @schema.AgentStream[Int32] = @schema.AgentStream::produce(
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
        @async-core.TaskCancelled::TaskCancelled => ()
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
        @async-core.TaskCancelled::TaskCancelled => ()
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

///|
test "detach during get rejects late delivery and disposes only after the host borrow settles" {
  for custom in [false, true] {
    for cancelled in [false, true] {
      reset_tool_client_resource_drop_counts()
      set_tool_client_host_mode(if custom { 5 } else { 4 })
      defer set_tool_client_host_mode(0)
      tool_client_reply_secret()
      run_tool_client_stream_async(() => {
        let client = ToolClient::new("terminal-test")
        defer client.drop()
        let invocation = start_output_invocation(client)
        @async-core.with_task_group(group => {
          let get = group.spawn(() => invocation.get())
          let (turn, next) = @async-core.Future::new()
          group.spawn_bg(() => ignore(next.complete(())))
          turn.get()
          assert_true(get.try_wait() is None)
          let queued_get = group.spawn(() => invocation.get())
          let (queued_turn, queued_next) = @async-core.Future::new()
          group.spawn_bg(() => ignore(queued_next.complete(())))
          queued_turn.get()
          invocation.detach_result_observer()
          invocation.detach_result_observer()
          assert_eq(tool_client_resource_drop_count(4), 0)
          assert_eq(tool_client_resource_drop_count(0), 0)
          assert_eq(tool_client_resource_drop_count(6), 0)
          if cancelled {
            get.cancel()
            invocation.cancel()
            assert_eq(tool_client_resource_drop_count(5), 1)
          }
          try get.wait() catch {
            @async-core.TaskCancelled::TaskCancelled => assert_true(cancelled)
            error => fail("unexpected result error: \{repr(error)}")
          } noraise {
            result => {
              assert_false(cancelled)
              assert_true(
                result is Err(Rpc(Protocol("result observer was detached"))),
              )
            }
          }
          assert_true(
            queued_get.wait()
            is Err(Rpc(Protocol("result observer was detached"))),
          )
        })
        assert_eq(tool_client_resource_drop_count(0), 1)
        assert_eq(tool_client_resource_drop_count(4), 1)
        invocation.cancel()
        assert_eq(
          tool_client_resource_drop_count(5),
          if cancelled {
            1
          } else {
            0
          },
        )
        invocation.drop()
      })
      assert_eq(tool_client_resource_drop_count(0), 1)
      assert_eq(tool_client_resource_drop_count(6), 1)
    }
  }
}

///|
test "detach releases unclaimed cached capabilities but not delivered capabilities" {
  for custom in [false, true] {
    for delivered in [false, true] {
      reset_tool_client_resource_drop_counts()
      set_tool_client_host_mode(if custom { 7 } else { 6 })
      defer set_tool_client_host_mode(0)
      tool_client_reply_secret()
      run_tool_client_stream_async(() => {
        let client = ToolClient::new("terminal-test")
        defer client.drop()
        let invocation = start_output_invocation(client)
        if delivered {
          match invocation.get() {
            Ok(result) => {
              assert_false(custom)
              @model.drop_wit_capabilities(result.result.unwrap().value)
            }
            Err(UnknownToolError(_, payload)) => {
              assert_true(custom)
              @model.drop_wit_capabilities(payload.value)
            }
            _ => fail("unexpected delivered result")
          }
          assert_eq(tool_client_resource_drop_count(0), 1)
        } else {
          // Reading output drives the result but does not claim its payload.
          assert_true(invocation.stdout.unwrap().read(1) is None)
          let (turn, next) = @async-core.Future::new()
          @async-core.with_task_group(group => {
            group.spawn_bg(() => ignore(next.complete(())))
            turn.get()
          })
          assert_eq(tool_client_resource_drop_count(0), 0)
        }
        invocation.detach_result_observer()
        invocation.drop()
        assert_eq(tool_client_resource_drop_count(0), 1)
        assert_eq(tool_client_resource_drop_count(4), 1)
      })
    }
  }
}

///|
test "permanent output close and result detach leave sibling bytes and invocation independent" {
  for detach in [false, true] {
    reset_tool_client_resource_drop_counts()
    set_tool_client_host_mode(8)
    defer set_tool_client_host_mode(0)
    set_tool_client_result_node(tool_client_bytes_pointer(b"ok"), 0, 0, 0)
    run_tool_client_stream_async(() => {
      let client = ToolClient::new("terminal-test")
      defer client.drop()
      let invocation : ToolInvocation[NoToolError] = match
        client.start(["stream"], empty_tool_input(), None, true, true, fn(
          _,
          _,
        ) {
          None
        }) {
        Ok(invocation) => invocation
        _ => fail("failed to start invocation with both outputs")
      }
      invocation.stdout.unwrap().drop_sync()
      if detach {
        invocation.detach_result_observer()
      }
      assert_eq(tool_client_resource_drop_count(5), 0)
      assert_eq(tool_client_resource_drop_count(6), 1)
      assert_eq(collect_tool_output(invocation.stderr), Ok(Some(b"ok")))
      assert_true(invocation.stdout.unwrap().read(1) is None)
      if !detach {
        assert_true(invocation.get() is Ok(_))
        invocation.cancel()
        assert_eq(tool_client_resource_drop_count(5), 1)
      }
      invocation.drop()
    })
    assert_eq(tool_client_resource_drop_count(4), 1)
    assert_eq(tool_client_resource_drop_count(6), 2)
  }
}

///|
test "background observation leaves custom error decoding and ownership for delivery" {
  for delivered in [false, true] {
    reset_tool_client_resource_drop_counts()
    set_tool_client_host_mode(7)
    defer set_tool_client_host_mode(0)
    tool_client_reply_secret()
    run_tool_client_stream_async(() => {
      let decoded = Ref(0)
      let client = ToolClient::new("terminal-test")
      defer client.drop()
      let invocation = match
        client.start(["stream"], empty_tool_input(), None, true, false, fn(
          _,
          payload,
        ) {
          decoded.val += 1
          Some(Ok(payload))
        }) {
        Ok(invocation) => invocation
        _ => fail("failed to start custom error invocation")
      }
      assert_true(invocation.stdout.unwrap().read(1) is None)
      let (turn, next) = @async-core.Future::new()
      @async-core.with_task_group(group => {
        group.spawn_bg(() => ignore(next.complete(())))
        turn.get()
      })
      assert_eq(decoded.val, 0)
      if delivered {
        guard invocation.get() is Err(Tool(payload)) else {
          fail("custom error was not delivered")
        }
        assert_true(invocation.get() is Err(Tool(_)))
        assert_eq(decoded.val, 1)
        invocation.detach_result_observer()
        assert_eq(tool_client_resource_drop_count(0), 0)
        @model.drop_wit_capabilities(payload.value)
      }
      invocation.drop()
      assert_eq(decoded.val, if delivered { 1 } else { 0 })
      assert_eq(tool_client_resource_drop_count(0), 1)
      assert_eq(tool_client_resource_drop_count(4), 1)
    })
  }
}
