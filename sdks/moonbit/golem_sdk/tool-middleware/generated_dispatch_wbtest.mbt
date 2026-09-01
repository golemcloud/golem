///|
fn generated_presented_tool(middleware_name : String) -> @toolCommon.Tool raise {
  let middleware = match get_registered_tool_middleware(middleware_name) {
    Ok(middleware) => middleware
    Err(_) =>
      raise Failure::Failure("generated test middleware is not registered")
  }
  match middleware.scope {
    @toolCommon.ToolMiddlewareScope::Monomorphic(scope) => scope.presented
    @toolCommon.ToolMiddlewareScope::Universal =>
      raise Failure::Failure("generated test middleware is not monomorphic")
  }
}

///|
fn generated_input(
  command_path : Array[String],
  params : Array[(String, @model.SchemaValue)],
) -> @types.TypedSchemaValue raise {
  @authoring.generated_input_for_tests(command_path, params)
}

///|
fn[T : @schema.IntoSchema] generated_result(
  value : T,
) -> @types.TypedSchemaValue raise {
  @model.typed_schema_value_to_wit(@schema.try_into_typed_schema_value(value))
}

///|
fn[T : @schema.FromSchema] generated_result_value(
  result : @toolCommon.InvocationResult,
) -> T raise {
  let decoded = match @toolCore.decode_invocation_result(result) {
    Ok(decoded) => decoded
    Err(message) => raise Failure::Failure(message)
  }
  match @toolCore.project_result_value(decoded) {
    Ok(value) => value
    Err(message) => raise Failure::Failure(message)
  }
}

///|
async fn invoke_generated(
  middleware_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  underlying : UnderlyingTool,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  invoke_registered_tool_middleware(
    middleware_name,
    "presented",
    generated_presented_tool(middleware_name),
    command_path,
    input,
    stdin,
    @agentCommon.Principal::Anonymous,
    underlying,
  )
}

///|
fn reset_generated_middlewares() -> Unit raise {
  clear_tool_middlewares_for_tests()
  @authoring.register_generated_middlewares_for_tests()
}

///|
#warnings("-unused_async")
fn counted_source_stream(drops : Ref[Int]) -> @async-core.Stream[Byte] {
  let emitted = Ref(false)
  let closed = Ref(false)
  let close = fn() {
    if !closed.val {
      closed.val = true
      drops.val = drops.val + 1
    }
  }
  @async-core.Stream::from_source(
    async fn(_) {
      if closed.val || emitted.val {
        None
      } else {
        emitted.val = true
        Some(([1, 2, 3] : FixedArray[Byte]))
      }
    },
    async fn() { close() },
    close,
  )
}

///|
#warnings("-unused_async")
fn directly_counted_source_stream(
  read : async (Int) -> FixedArray[Byte]?,
  close_attempts : Ref[Int],
) -> @async-core.Stream[Byte] {
  @async-core.Stream::from_source(
    read,
    async fn() { close_attempts.val = close_attempts.val + 1 },
    fn() { close_attempts.val = close_attempts.val + 1 },
  )
}

///|
fn projected_stdout_producer(
  source : @async-core.Stream[Byte],
) -> async (@async-core.Sink[Byte]) -> Unit {
  match transfer_aware_stdout(source).take_producer() {
    Some(producer) => producer
    None => abort("transfer-aware stdout did not expose its producer")
  }
}

///|
priv suberror ProjectedStdoutSourceFailure derive(Debug)

///|
#warnings("-unused_async")
test "generated monomorphic dispatch forwards, short-circuits, and retries" {
  reset_generated_middlewares()
  run_component_async(async fn() {
    let forward_calls = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, stdin) {
          forward_calls.val = forward_calls.val + 1
          assert_eq(path, ["value"])
          assert_true(stdin is None)
          Ok({ result: Some(generated_result(5)), stdout: None, })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["value"],
        generated_input(["value"], [
          ("value", @model.SchemaValue::String("hello")),
        ]),
        None,
        underlying,
      )
      match result {
        Ok(result) => {
          let value : Int = generated_result_value(result)
          assert_eq(value, 5)
        }
        Err(error) => fail("generated forwarding failed: \{Repr(error)}")
      }
    })
    assert_eq(forward_calls.val, 1)

    let short_circuit_calls = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(_, _, _) {
          short_circuit_calls.val = short_circuit_calls.val + 1
          Ok({ result: Some(generated_result(())), stdout: None, })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "adapter",
        ["unit"],
        generated_input(["unit"], []),
        None,
        underlying,
      )
      assert_true(result is Ok({ result: None, stdout: None, }))
    })
    assert_eq(short_circuit_calls.val, 0)

    let adapter_calls = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, stdin) {
          adapter_calls.val = adapter_calls.val + 1
          assert_eq(path, ["fetch"])
          assert_true(stdin is None)
          Ok({ result: Some(generated_result("stored")), stdout: None, })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "adapter",
        ["value"],
        generated_input(["value"], [
          ("value", @model.SchemaValue::String("key")),
        ]),
        None,
        underlying,
      )
      match result {
        Ok(result) => {
          let value : Int = generated_result_value(result)
          assert_eq(value, 6)
        }
        Err(error) => fail("generated adapter dispatch failed: \{Repr(error)}")
      }
    })
    assert_eq(adapter_calls.val, 1)

    let retry_calls = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, _) {
          retry_calls.val = retry_calls.val + 1
          assert_eq(path, ["retry"])
          Ok({ result: Some(generated_result(retry_calls.val)), stdout: None, })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["retry"],
        generated_input(["retry"], [
          ("value", @model.SchemaValue::String("again")),
        ]),
        None,
        underlying,
      )
      match result {
        Ok(result) => {
          let value : Int = generated_result_value(result)
          assert_eq(value, 2)
        }
        Err(error) => fail("generated retry failed: \{Repr(error)}")
      }
    })
    assert_eq(retry_calls.val, 2)
  })
}

///|
#warnings("-unused_async")
test "generated monomorphic dispatch rejects malformed input and result" {
  reset_generated_middlewares()
  run_component_async(async fn() {
    let malformed_input_calls = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(_, _, _) {
          malformed_input_calls.val = malformed_input_calls.val + 1
          Ok({ result: Some(generated_result(1)), stdout: None, })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["value"],
        raw_unit_value(),
        None,
        underlying,
      )
      assert_true(result is Err(@toolCommon.ToolError::InvalidInput(_)))
    })
    assert_eq(malformed_input_calls.val, 0)

    let malformed_result_calls = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(_, _, _) {
          malformed_result_calls.val = malformed_result_calls.val + 1
          Ok({ result: Some(generated_result(())), stdout: None, })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["value"],
        generated_input(["value"], [
          ("value", @model.SchemaValue::String("hello")),
        ]),
        None,
        underlying,
      )
      assert_true(result is Err(@toolCommon.ToolError::InvalidResult(_)))
    })
    assert_eq(malformed_result_calls.val, 1)
  })
}

///|
#warnings("-unused_async")
test "generated monomorphic dispatch decodes and re-encodes custom errors" {
  reset_generated_middlewares()
  let stdin_drops = Ref(0)
  run_component_async(async fn() {
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, stdin) {
          assert_eq(path, ["io"])
          match stdin {
            Some(stdin) => stdin.drop_sync()
            None => fail("generated custom-error dispatch lost stdin")
          }
          Err(
            @toolCommon.ToolError::CustomError(
              @authoring.run_error_payload_for_tests("boom"),
            ),
          )
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["io"],
        generated_input(["io"], []),
        Some(counted_stream(stdin_drops)),
        underlying,
      )
      match result {
        Err(@toolCommon.ToolError::CustomError(payload)) => {
          let decoded = @authoring.decode_run_error_payload_for_tests(payload)
          assert_true(decoded is @authoring.RunError::Broken("boom"))
        }
        _ => fail("generated custom error did not round-trip")
      }
    })

    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, stdin) {
          assert_eq(path, ["io"])
          match stdin {
            Some(stdin) => stdin.drop_sync()
            None => fail("generated malformed-error dispatch lost stdin")
          }
          Err(@toolCommon.ToolError::CustomError(generated_result(())))
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["io"],
        generated_input(["io"], []),
        Some(counted_stream(stdin_drops)),
        underlying,
      )
      assert_true(result is Err(@toolCommon.ToolError::InvalidResult(_)))
    })
  })
  assert_eq(stdin_drops.val, 2)
}

///|
#warnings("-unused_async")
test "generated monomorphic dispatch cleans abandoned stdout and transfers returned stdout" {
  reset_generated_middlewares()
  let abandoned_drops = Ref(0)
  run_component_async(async fn() {
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, _) {
          assert_eq(path, ["abandon-stdout"])
          Ok({ result: None, stdout: Some(counted_stream(abandoned_drops)), })
        },
        no_raw_drop,
        group,
      )
      let result = invoke_generated(
        "policy",
        ["abandon-stdout"],
        generated_input(["abandon-stdout"], []),
        None,
        underlying,
      )
      assert_true(
        result is Err(@toolCommon.ToolError::InvalidResult("abandoned stdout")),
      )
      assert_eq(abandoned_drops.val, 0)
    })
    assert_eq(abandoned_drops.val, 1)

    let returned_drops = Ref(0)
    let transferred_producer : Ref[(async (@async-core.Sink[Byte]) -> Unit)?] = Ref(
      None,
    )
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, _) {
          assert_eq(path, ["stdout"])
          Ok({ result: None, stdout: Some(counted_stream(returned_drops)), })
        },
        no_raw_drop,
        group,
      )
      match
        invoke_generated(
          "policy",
          ["stdout"],
          generated_input(["stdout"], []),
          None,
          underlying,
        ) {
        Ok(result) => {
          assert_true(result.result is None)
          match result.stdout {
            Some(stdout) => transferred_producer.val = stdout.take_producer()
            None => fail("generated stdout dispatch lost final stream")
          }
        }
        Err(error) => fail("generated stdout dispatch failed: \{Repr(error)}")
      }
      assert_eq(returned_drops.val, 0)
    })
    assert_eq(returned_drops.val, 0)
    match transferred_producer.val {
      Some(producer) => {
        let (stream, sink) = @async-core.Stream::new()
        producer(sink)
        stream.drop()
      }
      None => fail("generated stdout dispatch did not transfer its producer")
    }
    assert_eq(returned_drops.val, 0)
  })
}

///|
#warnings("-unused_async")
test "generated monomorphic dispatch relays lifted stdout after group teardown" {
  reset_generated_middlewares()
  let source_drops = Ref(0)
  let transferred_producer : Ref[(async (@async-core.Sink[Byte]) -> Unit)?] = Ref(
    None,
  )
  run_component_async(async fn() {
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(path, _, _) {
          assert_eq(path, ["stdout"])
          Ok({
            result: None,
            stdout: Some(counted_source_stream(source_drops)),
          })
        },
        no_raw_drop,
        group,
      )
      match
        invoke_generated(
          "policy",
          ["stdout"],
          generated_input(["stdout"], []),
          None,
          underlying,
        ) {
        Ok(result) =>
          match result.stdout {
            Some(stdout) => transferred_producer.val = stdout.take_producer()
            None => fail("generated stdout dispatch lost lifted source")
          }
        Err(error) => fail("generated stdout dispatch failed: \{Repr(error)}")
      }
      assert_eq(source_drops.val, 0)
    })
    assert_eq(source_drops.val, 0)
    match transferred_producer.val {
      Some(producer) => {
        let (stream, sink) = @async-core.Stream::new(capacity=64)
        producer(sink)
        match stream.read(64) {
          Some(data) => assert_eq(data, ([1, 2, 3] : FixedArray[Byte]))
          None => fail("lifted stdout relay produced no data")
        }
        assert_true(stream.read(64) is None)
      }
      None => fail("lifted stdout relay did not transfer its producer")
    }
    assert_eq(source_drops.val, 1)
  })
}

///|
#warnings("-unused_async")
test "transfer-aware stdout closes its source when the consumer closes early" {
  let close_attempts = Ref(0)
  run_component_async(async fn() {
    let producer = projected_stdout_producer(
      directly_counted_source_stream(
        async fn(_) { Some(([1, 2, 3] : FixedArray[Byte])) },
        close_attempts,
      ),
    )
    let (stream, sink) = @async-core.Stream::new(capacity=64)
    stream.drop()
    producer(sink)
    assert_eq(close_attempts.val, 1)
  })
}

///|
#warnings("-unused_async")
test "transfer-aware stdout closes its source when reading raises" {
  let close_attempts = Ref(0)
  run_component_async(async fn() {
    let producer = projected_stdout_producer(
      directly_counted_source_stream(
        async fn(_) { raise ProjectedStdoutSourceFailure },
        close_attempts,
      ),
    )
    let (stream, sink) = @async-core.Stream::new(capacity=64)
    try producer(sink) catch {
      ProjectedStdoutSourceFailure => ()
      _ => fail("unexpected transfer-aware stdout source error")
    } noraise {
      _ => fail("raising transfer-aware stdout source unexpectedly completed")
    }
    stream.drop()
    assert_eq(close_attempts.val, 1)
  })
}

///|
test "transfer-aware stdout closes its source when its producer is cancelled" {
  let close_attempts = Ref(0)
  run_component_async(async fn() {
    let (started, started_promise) = @async-core.Future::new()
    let (blocked, _blocked_promise) = @async-core.Future::new()
    let producer = projected_stdout_producer(
      directly_counted_source_stream(
        async fn(_) {
          ignore(started_promise.complete(()))
          blocked.get()
          None
        },
        close_attempts,
      ),
    )
    let (stream, sink) = @async-core.Stream::new(capacity=64)
    @async-core.with_task_group(async fn(group) {
      let task = group.spawn(async fn() { producer(sink) })
      started.get()
      task.cancel()
      try task.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        _ => fail("unexpected transfer-aware stdout cancellation error")
      } noraise {
        _ => fail("cancelled transfer-aware stdout unexpectedly completed")
      }
    })
    stream.drop()
    assert_eq(close_attempts.val, 1)
  })
}

///|
#warnings("-unused_async")
test "transfer-aware stdout closes an unstarted source exactly once" {
  let close_attempts = Ref(0)
  run_component_async(async fn() {
    let stdout = transfer_aware_stdout(
      directly_counted_source_stream(
        async fn(_) { Some(([1, 2, 3] : FixedArray[Byte])) },
        close_attempts,
      ),
    )
    stdout.drop()
    stdout.drop()
    assert_eq(close_attempts.val, 1)
  })
}
