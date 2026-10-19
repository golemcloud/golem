///|
extern "wasm" fn set_schema_value_stream_host_mode(mode : Int) = "golem:test" "set-schema-value-stream-host-mode"

///|
test "parameter failure rolls back an earlier decoded stream" {
  let first_source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(1),
  )
  let second_source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(2),
  )
  let fields = [
    @model.SchemaValue::Stream(first_source),
    @model.SchemaValue::Stream(second_source),
  ]
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) { dropped[0] += 1 },
    drop_native_stream: fn(_) {  },
  }
  let first : @schema.AgentStream[Int] = decode_param_with_resource_drops(
    fields, 0, "partial transfer", drops,
  )
  guard @schema.to_value_as(first) is @model.SchemaValue::Stream(first_owned) else {
    fail("expected the first stream to decode")
  }
  assert_true(first_source.is_present())
  assert_true(first_owned.is_present())
  let result : Result[Int, _] = try
    decode_param_with_resource_drops(fields, 1, "partial transfer", drops) |> Ok
  catch {
    e => Err(e)
  }
  guard result is Err(@common.AgentError::InvalidInput(_)) else {
    fail("decoding the second stream as Int must fail")
  }
  assert_false(first_owned.is_present())
  assert_false(first_source.is_present())
  assert_false(second_source.is_present())
  @model.SchemaValue::Stream(first_owned).release_resources(drops)
  @model.SchemaValue::Record(fields).release_resources(drops)
  assert_eq(dropped[0], 2)
}

///|
test "output encoding preserves wrap cancellation and cleanup" {
  @test_support.reset_resource_drop_counts()
  let producer_started = [false]
  let cleanup_runs = [0]
  @test_support.run_async(async fn() {
    set_schema_value_stream_host_mode(3)
    defer set_schema_value_stream_host_mode(0)
    let captured = @types.Secret::Secret(40)
    let output : @schema.AgentStream[Int] = @schema.AgentStream::produce(
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
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let encode = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        encode_output(output)
      })
      started.get()
      encode.cancel()
      try encode.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error => fail("dispatch changed cancellation category: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled dispatch encoding unexpectedly completed")
      }
    })
  })
  assert_false(producer_started[0])
  assert_eq(cleanup_runs[0], 1)
  assert_eq(@test_support.resource_drop_count(@test_support.SECRET_DROPS), 1)
}

///|
test "input encoding preserves wrap cancellation and cleanup" {
  @test_support.reset_resource_drop_counts()
  let producer_started = [false]
  let cleanup_runs = [0]
  @test_support.run_async(async fn() {
    set_schema_value_stream_host_mode(3)
    defer set_schema_value_stream_host_mode(0)
    let captured = @types.Secret::Secret(41)
    let input : @schema.AgentStream[Int] = @schema.AgentStream::produce(
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
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let encode = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        encode_invocation_input_async([@schema.to_value_as(input)])
      })
      started.get()
      encode.cancel()
      try encode.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error => fail("dispatch changed cancellation category: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled dispatch input encoding unexpectedly completed")
      }
    })
  })
  assert_false(producer_started[0])
  assert_eq(cleanup_runs[0], 1)
  assert_eq(@test_support.resource_drop_count(@test_support.SECRET_DROPS), 1)
}

///|
test "a failed input encode keeps argument permission cards" {
  @test_support.reset_resource_drop_counts()
  let card_a = @card.adopt(@types.PermissionCard::PermissionCard(613))
  let result = try? encode_invocation_input(
    @test_support.failing_card_input(card_a, 612),
  )
  assert_true(result is Err(_))
  assert_true(card_a.is_present())
  assert_eq(
    @test_support.resource_drop_count(@test_support.QUOTA_TOKEN_DROPS),
    1,
  )
  assert_eq(
    @test_support.resource_drop_count(@test_support.PERMISSION_CARD_DROPS),
    0,
  )
  let card_b = @card.adopt(@types.PermissionCard::PermissionCard(614))
  @test_support.run_async(async fn() {
    let result = try? encode_invocation_input_async(
      @test_support.failing_card_input(card_b, 615),
    )
    assert_true(result is Err(_))
  })
  assert_true(card_b.is_present())
  assert_eq(
    @test_support.resource_drop_count(@test_support.QUOTA_TOKEN_DROPS),
    2,
  )
  assert_eq(
    @test_support.resource_drop_count(@test_support.PERMISSION_CARD_DROPS),
    0,
  )
  ignore(@card.take(card_a))
  ignore(@card.take(card_b))
}
