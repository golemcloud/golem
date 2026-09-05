///|
fn test_schema(body : @model.SchemaTypeBody) -> SchemaRef {
  SchemaRef::new({ defs: [], root: @model.schema_type(body), })
}

///|
test "record JSON round-trips through reflected SchemaValue" {
  let metadata = @model.empty_metadata()
  let schema = test_schema(
    Record([
      { name: "message", body: @model.schema_type(String), metadata, },
      { name: "count", body: @model.schema_type(U32(None)), metadata, },
    ]),
  )
  let input = Json::object({
    "message": Json::string("hello"),
    "count": Json::number(3.0),
  })
  assert_eq(schema.unpack_json(schema.pack_json(input)), input)
}

///|
test "flags JSON uses names while SchemaValue uses positions" {
  let schema = test_schema(Flags(["read", "write", "admin"]))
  let input = Json::array([Json::string("write"), Json::string("admin")])
  assert_eq(
    schema.pack_json(input),
    @model.SchemaValue::Flags([false, true, true]),
  )
  assert_eq(schema.unpack_json(schema.pack_json(input)), input)
}

///|
test "record JSON rejects unknown fields" {
  let schema = test_schema(Record([]))
  try schema.pack_json(Json::object({ "extra": Json::number(1.0) })) catch {
    _ => ()
  } noraise {
    _ => fail("expected unknown field to be rejected")
  }
}

///|
test "stream detection follows nested and recursive definitions" {
  let metadata = @model.empty_metadata()
  let schema = SchemaRef::new({
    defs: [
      {
        id: "recursive",
        name: None,
        body: @model.schema_type(
          Record([
            {
              name: "next",
              body: @model.schema_type(Ref("recursive")),
              metadata,
            },
            {
              name: "events",
              body: @model.schema_type(Stream(Some(@model.schema_type(String)))),
              metadata,
            },
          ]),
        ),
      },
    ],
    root: @model.schema_type(Ref("recursive")),
  })
  assert_true(schema.contains_stream())
  assert_false(test_schema(List(@model.schema_type(String))).contains_stream())
}

///|
fn test_invocation(
  value : @model.SchemaValue?,
) -> Invocation[@model.SchemaValue?] {
  {
    metadata: {
      agent_id: {
        component_id: { uuid: { high_bits: 0UL, low_bits: 0UL, }, },
        value: "test-agent",
      },
      idempotency_key: "test-key",
    },
    value,
  }
}

///|
test "reflected output validation rejects missing and unexpected values" {
  let unit_method : AgentMethod = {
    name: "unit",
    description: "",
    prompt_hint: None,
    input: test_schema(Record([])),
    output: None,
  }
  let value_method = {
    ..unit_method,
    name: "value",
    output: Some(test_schema(String)),
  }
  try validate_invocation_output(value_method, test_invocation(None)) catch {
    Schema(message) =>
      assert_eq(message, "single-output method returned no value")
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected a missing output to be rejected")
  }
  try
    validate_invocation_output(
      unit_method,
      test_invocation(Some(@model.SchemaValue::String("unexpected"))),
    )
  catch {
    Schema(message) => assert_eq(message, "unit method returned a value")
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected an unexpected output to be rejected")
  }
}

///|
test "non-awaited reflected methods reject nested streams" {
  let entry : AgentMethod = {
    name: "streaming",
    description: "",
    prompt_hint: None,
    input: test_schema(Record([])),
    output: Some(test_schema(List(@model.schema_type(Stream(None))))),
  }
  try reject_non_awaited_streams(entry, "trigger") catch {
    Schema(message) =>
      assert_eq(
        message, "trigger is unavailable for streaming method 'streaming'",
      )
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected a streaming method to be rejected")
  }
}
