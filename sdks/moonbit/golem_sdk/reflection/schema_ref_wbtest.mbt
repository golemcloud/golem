///|
fn test_schema(body : @model.SchemaTypeBody) -> SchemaRef {
  SchemaRef::new({ defs: [], root: @model.schema_type(body), })
}

///|
test "JSON Schema projects records and recursive definitions" {
  let metadata = @model.empty_metadata()
  let schema = SchemaRef::new({
    defs: [
      {
        id: "pkg/node",
        name: None,
        body: @model.schema_type(
          Record([
            {
              name: "next",
              body: @model.schema_type(
                Option(@model.schema_type(Ref("pkg/node"))),
              ),
              metadata,
            },
          ]),
        ),
      },
    ],
    root: @model.schema_type(Ref("pkg/node")),
  })
  assert_eq(
    schema.to_json_schema(),
    Json::object({
      "$ref": Json::string("#/$defs/pkg~1node"),
      "$schema": Json::string("https://json-schema.org/draft/2020-12/schema"),
      "$defs": Json::object({
        "pkg/node": Json::object({
          "type": Json::string("object"),
          "properties": Json::object({
            "next": Json::object({
              "oneOf": Json::array([
                Json::object({ "type": Json::string("null") }),
                Json::object({ "$ref": Json::string("#/$defs/pkg~1node") }),
              ]),
            }),
          }),
          "required": Json::array([]),
          "additionalProperties": Json::boolean(false),
        }),
      }),
    }),
  )
}

///|
test "canonical JSON rejects cyclic alias references" {
  let schema = SchemaRef::new({
    defs: [
      { id: "alias-a", name: None, body: @model.schema_type(Ref("alias-b")), },
      { id: "alias-b", name: None, body: @model.schema_type(Ref("alias-a")), },
    ],
    root: @model.schema_type(Ref("alias-a")),
  })
  try schema.pack_json(Json::null()) catch {
    Schema(message) => assert_eq(message, "cyclic schema reference 'alias-a'")
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected cyclic alias rejection")
  }
}

///|
test "JSON Schema retains text and binary constraints" {
  let text = test_schema(
    Text({
      languages: Some(["en", "de"]),
      min_length: Some(2U),
      max_length: Some(8U),
      regex: Some("^[a-z]+$"),
    }),
  )
  assert_eq(
    text.to_json_schema(include_draft_marker=false),
    Json::object({
      "type": Json::string("object"),
      "properties": Json::object({
        "text": Json::object({
          "type": Json::string("string"),
          "minLength": Json::number(2.0),
          "maxLength": Json::number(8.0),
          "pattern": Json::string("^[a-z]+$"),
        }),
        "language": Json::object({
          "type": Json::string("string"),
          "enum": Json::array([Json::string("en"), Json::string("de")]),
        }),
      }),
      "required": Json::array([Json::string("text")]),
      "additionalProperties": Json::boolean(false),
    }),
  )
  let binary = test_schema(
    Binary({
      mime_types: Some(["image/png"]),
      min_bytes: Some(3U),
      max_bytes: Some(6U),
    }),
  )
  let rendered = binary.to_json_schema(include_draft_marker=false)
  assert_eq(
    rendered,
    Json::object({
      "type": Json::string("object"),
      "properties": Json::object({
        "bytes": Json::object({
          "type": Json::string("string"),
          "contentEncoding": Json::string("base64url"),
          "pattern": Json::string(
            "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$",
          ),
          "minLength": Json::number(4.0),
          "maxLength": Json::number(8.0),
        }),
        "mimeType": Json::object({
          "type": Json::string("string"),
          "enum": Json::array([Json::string("image/png")]),
        }),
      }),
      "required": Json::array([Json::string("bytes")]),
      "additionalProperties": Json::boolean(false),
    }),
  )
  let pattern = match rendered {
    Object(fields) =>
      match fields.get("properties") {
        Some(Object(properties)) =>
          match properties.get("bytes") {
            Some(Object(bytes)) =>
              match bytes.get("pattern") {
                Some(String(value)) => value
                _ => fail("expected binary pattern string")
              }
            _ => fail("expected binary bytes schema")
          }
        _ => fail("expected binary properties")
      }
    _ => fail("expected binary JSON Schema object")
  }
  let regex = @string.Regex::Regex(pattern.replace_all(old="_-]", new="_\\-]"))
  for valid in ["", "AQ", "AQI", "AQID", "-_8"] {
    assert_true(regex.execute(valid) is Some(_))
  }
  for invalid in ["+/8", "AQ==", "-_9", "A"] {
    assert_true(regex.execute(invalid) is None)
  }
}

///|
test "JSON Schema keeps wide numeric bounds exact in metadata" {
  let schema = test_schema(
    U64(
      Some({
        min: Some(@types.NumericBound::Unsigned(9007199254740993UL)),
        max: Some(@types.NumericBound::Unsigned(18446744073709551615UL)),
        unit: None,
      }),
    ),
  )
  let rendered = schema.to_json_schema(include_draft_marker=false)
  let fields = match rendered {
    Object(fields) => fields
    _ => fail("expected JSON Schema object")
  }
  assert_eq(fields.get("type"), Some(Json::string("string")))
  assert_eq(fields.get("format"), Some(Json::string("uint64")))
  assert_eq(fields.get("pattern"), Some(Json::string("^(?:0|[1-9][0-9]*)$")))
  assert_eq(
    fields.get("x-golem-minimum"),
    Some(Json::string("9007199254740993")),
  )
  assert_eq(
    fields.get("x-golem-maximum"),
    Some(Json::string("18446744073709551615")),
  )
}

///|
test "JSON Schema projects union discriminators and metadata" {
  let metadata = {
    ..@model.empty_metadata(),
    doc: Some("event payload"),
    aliases: ["evt"],
  }
  let schema = SchemaRef::new({
    defs: [],
    root: @model.schema_type(
      Union([
        {
          tag: "event",
          body: @model.schema_type(String),
          discriminator: @types.DiscriminatorRule::Prefix("a.b"),
          metadata: @model.empty_metadata(),
        },
      ]),
      metadata~,
    ),
  })
  assert_eq(
    schema.to_json_schema(include_draft_marker=false),
    Json::object({
      "oneOf": Json::array([
        Json::object({
          "allOf": Json::array([
            Json::object({ "type": Json::string("string") }),
            Json::object({ "pattern": Json::string("^a\\.b") }),
          ]),
        }),
      ]),
      "description": Json::string("event payload"),
      "x-golem-aliases": Json::array([Json::string("evt")]),
    }),
  )
}

///|
test "JSON Schema exposes path URL and quantity restrictions" {
  let path = test_schema(
    Path({
      direction: @types.PathDirection::INPUT,
      kind: @types.PathKind::FILE,
      allowed_mime_types: Some(["image/png"]),
      allowed_extensions: Some(["png"]),
    }),
  )
  assert_eq(
    path.to_json_schema(include_draft_marker=false),
    Json::object({
      "type": Json::string("string"),
      "format": Json::string("file-path"),
      "title": Json::string("input file path"),
      "description": Json::string(
        "Allowed extensions: png; Allowed MIME types: image/png",
      ),
    }),
  )
  let url = test_schema(
    Url({
      allowed_schemes: Some(["https"]),
      allowed_hosts: Some(["example.com"]),
    }),
  )
  assert_eq(
    url.to_json_schema(include_draft_marker=false),
    Json::object({
      "type": Json::string("string"),
      "format": Json::string("uri"),
      "title": Json::string("URL"),
      "description": Json::string(
        "Allowed schemes: https; Allowed hosts: example.com",
      ),
    }),
  )
  let quantity = test_schema(
    Quantity({
      base_unit: "m",
      allowed_suffixes: ["cm"],
      min: Some({ mantissa: 1L, scale: 0, unit: "m", }),
      max: None,
    }),
  )
  let rendered = quantity.to_json_schema(include_draft_marker=false)
  let fields = match rendered {
    Object(fields) => fields
    _ => fail("expected JSON Schema object")
  }
  assert_eq(fields.get("title"), Some(Json::string("Quantity (m)")))
  assert_eq(
    fields.get("x-golem-allowed-suffixes"),
    Some(Json::array([Json::string("cm")])),
  )
  assert_eq(
    fields.get("x-golem-minimum-quantity"),
    Some(
      Json::object({
        "mantissa": Json::string("1"),
        "scale": Json::number(0.0),
        "unit": Json::string("m"),
      }),
    ),
  )
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
test "64-bit JSON uses full-range canonical decimal strings" {
  let signed = test_schema(S64(None))
  let unsigned = test_schema(U64(None))
  assert_eq(
    signed.unpack_json(@model.SchemaValue::S64(-9223372036854775807L - 1L)),
    Json::string("-9223372036854775808"),
  )
  assert_eq(
    signed.pack_json(Json::string("9223372036854775807")),
    @model.SchemaValue::S64(9223372036854775807L),
  )
  assert_eq(
    unsigned.unpack_json(@model.SchemaValue::U64(18446744073709551615UL)),
    Json::string("18446744073709551615"),
  )
  assert_eq(
    unsigned.pack_json(Json::string("18446744073709551615")),
    @model.SchemaValue::U64(18446744073709551615UL),
  )
  for invalid in ["+1", "01", "-0", "9223372036854775808"] {
    try signed.pack_json(Json::string(invalid)) catch {
      _ => ()
    } noraise {
      _ => fail("expected malformed or overflowing s64 to be rejected")
    }
  }
  try unsigned.pack_json(Json::number(1.0)) catch {
    _ => ()
  } noraise {
    _ => fail("expected numeric u64 JSON to be rejected")
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
test "quantity JSON uses a full-range canonical mantissa string" {
  let schema = test_schema(
    Quantity({ base_unit: "kg", allowed_suffixes: [], min: None, max: None, }),
  )
  let value = @model.SchemaValue::Quantity({
    mantissa: -9223372036854775807L - 1L,
    scale: 2,
    unit: "kg",
  })
  let json = schema.unpack_json(value)
  assert_eq(
    json,
    Json::object({
      "mantissa": Json::string("-9223372036854775808"),
      "scale": Json::number(2.0),
      "unit": Json::string("kg"),
    }),
  )
  assert_eq(schema.pack_json(json), value)
  try
    schema.pack_json(
      Json::object({
        "mantissa": Json::string("-0"),
        "scale": Json::number(0.0),
        "unit": Json::string("kg"),
      }),
    )
  catch {
    _ => ()
  } noraise {
    _ => fail("expected non-canonical quantity mantissa to be rejected")
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
test "duration JSON uses a signed decimal nanoseconds object" {
  let schema = test_schema(Duration)
  let value = @model.SchemaValue::Duration(-9223372036854775807L - 1L)
  let json = Json::object({
    "nanoseconds": Json::string("-9223372036854775808"),
  })
  assert_eq(schema.unpack_json(value), json)
  assert_eq(schema.pack_json(json), value)
  try schema.pack_json(Json::string("PT1S")) catch {
    _ => ()
  } noraise {
    _ => fail("expected non-canonical duration JSON to be rejected")
  }
  try
    schema.pack_json(Json::object({ "nanoseconds": Json::string("-0") }))
  catch {
    _ => ()
  } noraise {
    _ => fail("expected non-canonical duration integer to be rejected")
  }
}

///|
test "omitted option record fields decode as none" {
  let metadata = @model.empty_metadata()
  let schema = SchemaRef::new({
    defs: [
      {
        id: "optional-string",
        name: None,
        body: @model.schema_type(Option(@model.schema_type(String))),
      },
    ],
    root: @model.schema_type(
      Record([
        {
          name: "direct",
          body: @model.schema_type(Option(@model.schema_type(String))),
          metadata,
        },
        {
          name: "referenced",
          body: @model.schema_type(Ref("optional-string")),
          metadata,
        },
      ]),
    ),
  })
  let absent = @model.SchemaValue::Record([
    @model.SchemaValue::Option(None),
    @model.SchemaValue::Option(None),
  ])
  assert_eq(schema.pack_json(Json::object({})), absent)
  let rendered = schema.to_json_schema(include_draft_marker=false)
  guard rendered is Object(fields) else { fail("expected schema object") }
  assert_eq(fields.get("required"), Some(Json::array([])))
}

///|
test "reflection JSON Schema rejects unrepresentable leaves" {
  for
    body in [
      @model.SchemaTypeBody::Future(None),
      @model.SchemaTypeBody::Stream(None),
      @model.SchemaTypeBody::QuotaToken({ resource_name: None, }),
    ] {
    assert_eq(
      test_schema(body).to_json_schema(include_draft_marker=false),
      Json::object({ "not": Json::object({}) }),
    )
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
