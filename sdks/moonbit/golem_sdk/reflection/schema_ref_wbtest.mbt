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
test "unsigned 32-bit JSON preserves values above the signed range" {
  let schema = test_schema(U32(None))
  assert_eq(
    schema.unpack_json(@model.SchemaValue::U32(4294967295U)),
    Json::number(4294967295.0),
  )
}

///|
test "64-bit JSON rejects values outside the safe integer range" {
  let signed = test_schema(S64(None))
  let unsigned = test_schema(U64(None))
  try signed.unpack_json(@model.SchemaValue::S64(9007199254740992L)) catch {
    _ => ()
  } noraise {
    _ => fail("expected unsafe signed integer to be rejected")
  }
  try unsigned.unpack_json(@model.SchemaValue::U64(9007199254740992UL)) catch {
    _ => ()
  } noraise {
    _ => fail("expected unsafe unsigned integer to be rejected")
  }
}

///|
test "native 64-bit refinements compare exact integer boundaries" {
  let signed = test_schema(
    S64(
      Some({
        min: Some(@types.NumericBound::Signed(9007199254740993L)),
        max: Some(@types.NumericBound::Signed(9007199254740993L)),
        unit: None,
      }),
    ),
  )
  signed.validate_value(@model.SchemaValue::S64(9007199254740993L))
  try signed.validate_value(@model.SchemaValue::S64(9007199254740992L)) catch {
    Schema(_) => ()
    _ => fail("expected numeric refinement rejection")
  } noraise {
    _ => fail("expected exact lower bound rejection")
  }
  let unsigned = test_schema(
    U64(
      Some({
        min: Some(@types.NumericBound::Unsigned(18446744073709551615UL)),
        max: None,
        unit: None,
      }),
    ),
  )
  unsigned.validate_value(@model.SchemaValue::U64(18446744073709551615UL))
  try
    unsigned.validate_value(@model.SchemaValue::U64(18446744073709551614UL))
  catch {
    Schema(_) => ()
    _ => fail("expected numeric refinement rejection")
  } noraise {
    _ => fail("expected exact unsigned lower bound rejection")
  }
}

///|
test "native path URL and quantity refinements reject invalid values" {
  let path = test_schema(
    Path({
      direction: @types.PathDirection::INPUT,
      kind: @types.PathKind::FILE,
      allowed_mime_types: None,
      allowed_extensions: Some(["txt"]),
    }),
  )
  path.validate_value(@model.SchemaValue::Path("report.txt"))
  try path.validate_value(@model.SchemaValue::Path("report.csv")) catch {
    Schema(_) => ()
    _ => fail("expected path extension rejection")
  } noraise {
    _ => fail("expected path extension rejection")
  }
  let url = test_schema(
    Url({
      allowed_schemes: Some(["https"]),
      allowed_hosts: Some(["example.com"]),
    }),
  )
  url.validate_value(@model.SchemaValue::Url("https://example.com/path"))
  try
    url.validate_value(@model.SchemaValue::Url("http://example.com/path"))
  catch {
    Schema(_) => ()
    _ => fail("expected URL scheme rejection")
  } noraise {
    _ => fail("expected URL scheme rejection")
  }
  let quantity = test_schema(
    Quantity({
      base_unit: "kg",
      allowed_suffixes: [],
      min: Some({ mantissa: 10L, scale: 0, unit: "kg", }),
      max: Some({ mantissa: 20L, scale: 0, unit: "kg", }),
    }),
  )
  quantity.validate_value(
    @model.SchemaValue::Quantity({ mantissa: 15L, scale: 0, unit: "kg", }),
  )
  try
    quantity.validate_value(
      @model.SchemaValue::Quantity({ mantissa: 21L, scale: 0, unit: "kg", }),
    )
  catch {
    Schema(_) => ()
    _ => fail("expected quantity maximum rejection")
  } noraise {
    _ => fail("expected quantity maximum rejection")
  }
}

///|
test "binary JSON round-trips unpadded base64url and MIME type" {
  let schema = test_schema(
    Binary({ min_bytes: None, max_bytes: None, mime_types: None, }),
  )
  let value = @model.SchemaValue::Binary(
    ([0, 251, 255] : FixedArray[Byte]),
    Some("image/png"),
  )
  let json = schema.unpack_json(value)
  assert_eq(
    json,
    Json::object({
      "bytes": Json::string("APv_"),
      "mimeType": Json::string("image/png"),
    }),
  )
  assert_eq(schema.pack_json(json), value)
  try schema.pack_json(Json::object({ "bytes": Json::string("APv_=") })) catch {
    _ => ()
  } noraise {
    _ => fail("expected padded base64url to be rejected")
  }
}

///|
test "quantity JSON round-trips within the safe integer range" {
  let schema = test_schema(
    Quantity({ base_unit: "kg", allowed_suffixes: [], min: None, max: None, }),
  )
  let value = @model.SchemaValue::Quantity({
    mantissa: 1234L,
    scale: 2,
    unit: "kg",
  })
  assert_eq(schema.pack_json(schema.unpack_json(value)), value)
  try
    schema.unpack_json(
      @model.SchemaValue::Quantity({
        mantissa: 9007199254740992L,
        scale: 0,
        unit: "kg",
      }),
    )
  catch {
    _ => ()
  } noraise {
    _ => fail("expected unsafe quantity mantissa to be rejected")
  }
}

///|
test "datetime JSON round-trips UTC instants and rejects invalid dates" {
  let schema = test_schema(Datetime)
  let instant = Json::string("1970-01-01T00:00:00.123456789Z")
  assert_eq(
    schema.pack_json(instant),
    @model.SchemaValue::Datetime({ seconds: 0L, nanoseconds: 123456789U, }),
  )
  assert_eq(schema.unpack_json(schema.pack_json(instant)), instant)
  assert_eq(
    schema.unpack_json(
      @model.SchemaValue::Datetime({ seconds: -1L, nanoseconds: 0U, }),
    ),
    Json::string("1969-12-31T23:59:59Z"),
  )
  assert_eq(
    schema.unpack_json(
      @model.SchemaValue::Datetime({ seconds: -62167219200L, nanoseconds: 0U, }),
    ),
    Json::string("0000-01-01T00:00:00Z"),
  )
  try schema.pack_json(Json::string("2025-02-29T00:00:00Z")) catch {
    _ => ()
  } noraise {
    _ => fail("expected invalid civil date to be rejected")
  }
}

///|
test "duration JSON round-trips ISO strings and accepts shorthand" {
  let schema = test_schema(Duration)
  let value = @model.SchemaValue::Duration(-90061000000001L)
  let json = Json::string("-P1DT1H1M1.000000001S")
  assert_eq(schema.unpack_json(value), json)
  assert_eq(schema.pack_json(json), value)
  assert_eq(
    schema.pack_json(Json::string("1500ms")),
    @model.SchemaValue::Duration(1500000000L),
  )
  assert_eq(
    schema.pack_json(Json::string("-9223372036854775808ns")),
    @model.SchemaValue::Duration(-9223372036854775807L - 1L),
  )
  assert_eq(
    schema.pack_json(Json::object({ "nanoseconds": Json::number(42.0) })),
    @model.SchemaValue::Duration(42L),
  )
  try schema.pack_json(Json::string("PT9223372037S")) catch {
    _ => ()
  } noraise {
    _ => fail("expected duration overflow to be rejected")
  }
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
      agent_id: { value: "test-agent", },
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
  try
    validate_invocation_output(
      value_method,
      test_invocation(Some(@model.SchemaValue::U32(7U))),
    )
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected a malformed output to be rejected")
  }
}

///|
test "schema-native reflected values reject wrong nested types" {
  let schema = test_schema(
    Record([
      {
        name: "items",
        body: @model.schema_type(List(@model.schema_type(U32(None)))),
        metadata: @model.empty_metadata(),
      },
    ]),
  )
  schema.validate_value(
    @model.SchemaValue::Record([
      @model.SchemaValue::List([@model.SchemaValue::U32(3U)]),
    ]),
  )
  try
    schema.validate_value(
      @model.SchemaValue::Record([
        @model.SchemaValue::List([@model.SchemaValue::String("wrong")]),
      ]),
    )
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected local schema-native validation failure")
  }
}

///|
test "declared nested numeric refinements apply to native and JSON values" {
  let restricted = @model.schema_type(
    U32(
      Some({
        min: Some(@types.Unsigned(5UL)),
        max: Some(@types.Unsigned(10UL)),
        unit: None,
      }),
    ),
  )
  let schema = test_schema(
    Record([
      { name: "count", body: restricted, metadata: @model.empty_metadata(), },
    ]),
  )
  schema.validate_value(
    @model.SchemaValue::Record([@model.SchemaValue::U32(7U)]),
  )
  assert_eq(
    schema.pack_json(Json::object({ "count": Json::number(7.0) })),
    @model.SchemaValue::Record([@model.SchemaValue::U32(7U)]),
  )
  try
    schema.validate_value(
      @model.SchemaValue::Record([@model.SchemaValue::U32(3U)]),
    )
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected native refinement rejection")
  }
  try schema.pack_json(Json::object({ "count": Json::number(3.0) })) catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected JSON refinement rejection")
  }
  try
    schema.unpack_json(
      @model.SchemaValue::Record([@model.SchemaValue::U32(3U)]),
    )
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected declared output refinement rejection")
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
