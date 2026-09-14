///|
fn cleanup_drops() -> @model.SchemaValueResourceDrops {
  {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  }
}

///|
fn cleanup_string_graph() -> @model.SchemaGraph raise {
  @schema.schema_graph_of_tag((@schema.type_tag() : @schema.TypeTag[String]))
}

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
  guard result is Err(@toolCommon.ToolError::InvalidInput(_)) else {
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
test "canonical field rejection releases undecoded streams" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(3),
  )
  let fields : Array[CanonicalInputValue] = [
    {
      name: "stream",
      aliases: [],
      short: None,
      type_: cleanup_string_graph(),
      value: @model.SchemaValue::Stream(source),
    },
  ]
  let result : Result[Int, _] = try
    decode_canonical_field_with_resource_drops(
      fields,
      "missing",
      "canonical cleanup",
      cleanup_drops(),
    )
    |> Ok
  catch {
    e => Err(e)
  }
  guard result is Err(@toolCommon.ToolError::InvalidInput(_)) else {
    fail("a missing canonical field must fail")
  }
  assert_false(source.is_present())
}

///|
test "decoded invocation rejection releases unforwarded streams" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(4),
  )
  release_canonical_input_values(
    [
      {
        name: "stream",
        aliases: [],
        short: None,
        type_: cleanup_string_graph(),
        value: @model.SchemaValue::Stream(source),
      },
    ],
    cleanup_drops(),
  )
  assert_false(source.is_present())
}

///|
test "wire invocation rejection drops stream resources" {
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) { dropped[0] += 1 },
    drop_native_stream: fn(_) {  },
  }
  let input : @types.TypedSchemaValue = {
    graph: @model.schema_graph_to_wit(cleanup_string_graph()),
    value: {
      value_nodes: [
        @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(14)),
      ],
      root: 0,
    },
  }
  release_wire_invocation_input(input, drops)
  assert_eq(dropped[0], 1)
}

///|
test "decoded invocation rejection calls configured stream drop" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(15),
  )
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) { dropped[0] += 1 },
    drop_native_stream: fn(_) {  },
  }
  release_canonical_input_values(
    [
      {
        name: "global",
        aliases: [],
        short: None,
        type_: cleanup_string_graph(),
        value: @model.SchemaValue::Stream(source),
      },
    ],
    drops,
  )
  assert_false(source.is_present())
  assert_eq(dropped[0], 1)
}
