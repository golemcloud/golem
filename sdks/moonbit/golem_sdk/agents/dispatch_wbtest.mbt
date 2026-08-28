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
  let result : Result[Int, _] = try? decode_param_with_resource_drops(
    fields, 1, "partial transfer", drops,
  )
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
