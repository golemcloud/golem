// Phase 3g tests for the `AgentError::CustomError` end-to-end path.
//
// Unlike the quota/rpc helpers, none of these touch a WASM host import: the
// schema encode/decode codec is pure, so the full construct → carrier → decode
// round-trip runs host-free.

///|
fn test_resource_drops() -> @model.SchemaValueResourceDrops {
  {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  }
}

///|
fn[T : @schema.FromSchema] decode_custom_error_for_test(
  error : @common.AgentError,
) -> T? raise {
  decode_custom_error_with_resource_drops(error, test_resource_drops())
}

///|
fn[T : @schema.IntoSchema] custom_error_of_for_test(
  value : T,
) -> @common.AgentError raise {
  let typed = @schema.try_into_typed_schema_value(value)
  @common.AgentError::CustomError(@model.typed_schema_value_to_wit(typed))
}

///|
test "custom_error round-trips a string message" {
  let err = custom_error("boom")
  guard err is CustomError(_) else { fail("expected CustomError") }
  let decoded : String? = decode_custom_error_for_test(err)
  debug_inspect(
    decoded,
    content=(
      #|Some("boom")
    ),
  )
}

///|
test "internal_error prefixes and round-trips" {
  let err = internal_error("disk full")
  let decoded : String? = decode_custom_error_for_test(err)
  debug_inspect(
    decoded,
    content=(
      #|Some("Internal error: disk full")
    ),
  )
}

///|
test "custom_error_of round-trips a typed (non-string) value" {
  let err = custom_error_of_for_test((42 : Int))
  guard err is CustomError(_) else { fail("expected CustomError") }
  let decoded : Int? = decode_custom_error_for_test(err)
  debug_inspect(decoded, content="Some(42)")
}

///|
test "failed custom error conversion drops its decoded stream" {
  let handle = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(90),
  )
  let stream : @schema.AgentStream[Int] = @schema.from_value_as(
    @model.SchemaValue::Stream(handle),
  )
  let error = custom_error_of_for_test(stream)
  let dropped = [0]
  let result : Result[Int?, _] = try
    decode_custom_error_with_resource_drops(error, {
      drop_secret: fn(_) {  },
      drop_quota_token: fn(_) {  },
      drop_stream: fn(_) { dropped[0] += 1 },
      drop_native_stream: fn(_) {  },
    })
    |> Ok
  catch {
    e => Err(e)
  }
  guard result is Err(_) else {
    fail("decoding a stream custom error as Int must fail")
  }
  assert_eq(dropped[0], 1)
}

///|
test "custom_error carries a self-describing schema graph" {
  let err = custom_error("x")
  guard err is CustomError(typed) else { fail("expected CustomError") }
  // The payload must carry a non-empty schema graph alongside the value tree
  // (guards against a "value only, missing schema" encoding regression).
  debug_inspect(typed.graph.type_nodes.length() > 0, content="true")
}

///|
test "invalid_* helpers build the matching variants" {
  debug_inspect(
    invalid_input_error("bad"),
    content=(
      #|InvalidInput("bad")
    ),
  )
  debug_inspect(
    invalid_method_error("nope"),
    content=(
      #|InvalidMethod("nope")
    ),
  )
  debug_inspect(
    invalid_type_error("wrong"),
    content=(
      #|InvalidType("wrong")
    ),
  )
}

///|
test "decode_custom_error returns None for non-custom errors" {
  let decoded : String? = decode_custom_error_for_test(invalid_input_error("x"))
  debug_inspect(decoded, content="None")
}

///|
test "decode_custom_error raises on a type mismatch" {
  let err = custom_error("not a number")
  let r : Result[Int?, _] = try decode_custom_error_for_test(err) |> Ok catch {
    e => Err(e)
  }
  guard r is Err(_) else { fail("expected a decode error") }
}
