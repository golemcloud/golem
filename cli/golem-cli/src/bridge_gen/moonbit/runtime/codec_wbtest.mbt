///|
fn round_trip(v : SchemaValue) -> SchemaValue raise {
  let s = schema_value_to_json(v).stringify()
  @debug.assert_eq(@json.parse(schema_value_render(v)), @json.parse(s))
  schema_value_from_json(@json.parse(s))
}

///|
test "direct writer preserves all optional payloads and numeric tokens" {
  let cases : Array[SchemaValue] = [
    F32Value(-0.0),
    F64Value(-0.0),
    F32Value(0.1),
    F32Value(1.0 / 0.0),
    F64Value(-1.0 / 0.0),
    F64Value(0.0 / 0.0),
    StringValue("\"\\\n\t\u0001é😀"),
    UuidValue(Uuid::from_bits(0x0123456789abcdefUL, 0xfedcba9876543210UL)),
    FixedListValue([U8Value(7)]),
    VariantValue(1, None),
    VariantValue(0, Some(StringValue("x"))),
    EnumValue(2),
    FlagsValue([true, false]),
    OptionValue(None),
    OptionValue(Some(BoolValue(false))),
    ResultValue(ResultOk(None)),
    ResultValue(ResultErr(Some(S32Value(-3)))),
    TextValue("text", None),
    TextValue("text", Some("en")),
    BinaryValue(b"\x00\xff", None),
    BinaryValue(b"\x00\xff", Some("image/png")),
    QuantityValue(-7L, 3, "unit"),
    StreamReferenceValue(None, None),
    StreamReferenceValue(Some("p"), None),
    StreamReferenceValue(None, Some("t")),
    StreamReferenceValue(Some("p"), Some("t")),
    UnionValue("tag", ListValue([S32Value(8)])),
  ]
  for value in cases {
    @debug.assert_eq(
      @json.parse(schema_value_render(value)),
      @json.parse(schema_value_to_json(value).stringify()),
    )
  }
  assert_eq(
    schema_value_render(U64Value(18446744073709551615UL)),
    "{\"kind\":\"u64\",\"value\":\"18446744073709551615\"}",
  )
  assert_eq(
    schema_value_render(F64Value(-0.0)),
    "{\"kind\":\"f64\",\"value\":-0}",
  )
}

///|
test "direct requests preserve configuration and optional members" {
  let parameters = RecordValue([StringValue("é😀\n")])
  let config : Array[AgentConfigEntry] = [
    {
      path: ["key"],
      value: @json.parse("{\"$float\":\"ordinary\",\"x\":null,\"z\":-0}"),
    },
  ]
  let env = [("A", "1"), ("B", "é")]
  for keyed in [false, true] {
    @debug.assert_eq(
      @json.parse(render_create_agent_request(parameters, config, env, keyed)),
      encode_create_agent_request(parameters, config, env, keyed),
    )
  }
  @debug.assert_eq(
    @json.parse(render_agent_key_request(parameters)),
    obj([("parameters", schema_value_to_json(parameters))]),
  )
  let agent_id = AgentId::parse("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d")
  let targets : Array[(AgentId?, SchemaValue?, String?)] = [
    (None, Some(parameters), None),
    (Some(agent_id), None, None),
    (Some(agent_id), None, Some("1a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d")),
  ]
  for target in targets {
    let (id, keyed_parameters, fingerprint) = target
    for schedule in [None, Some("later")] {
      @debug.assert_eq(
        @json.parse(
          render_agent_invocation_request(
            id,
            keyed_parameters,
            "method",
            ListValue([U8Value(251)]),
            "await",
            schedule,
            fingerprint,
          ),
        ),
        encode_agent_invocation_request(
          id,
          keyed_parameters,
          "method",
          ListValue([U8Value(251)]),
          "await",
          schedule,
          fingerprint,
        ),
      )
    }
  }
  for target in [(None, None), (Some(agent_id), Some(parameters))] {
    assert_true(
      raises(() => {
        ignore(
          render_agent_invocation_request(
            target.0,
            target.1,
            "method",
            RecordValue([]),
            "await",
            None,
            None,
          ),
        )
      }),
    )
  }
}

///|
test "agent references and environments decode from the server shapes" {
  let reference = decode_agent_reference(
    @json.parse(
      "{\"agentId\":\"0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d\",\"agentFingerprint\":\"f\"}",
    ),
  )
  assert_eq(reference.agent_id.to_string(), "0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d")
  assert_eq(reference.agent_fingerprint, "f")
  assert_true(
    raises(() => {
      ignore(
        decode_agent_reference(
          @json.parse("{\"agentId\":\"agent\",\"agentFingerprint\":\"f\"}"),
        ),
      )
    }),
  )
  @debug.assert_eq(
    decode_agent_metadata(
      @json.parse("{\"agentTypeName\":\"Counter\",\"fingerprint\":\"f\"}"),
    ),
    (Some("Counter"), "f"),
  )
  let page = @json.parse(
    (
      #|{"values":[
      #|{"account":{"id":"other"},"application":{"name":"app"},"environment":{"id":"e1","name":"env"}},
      #|{"account":{"id":"me"},"application":{"name":"app"},"environment":{"id":"e2","name":"other"}},
      #|{"account":{"id":"me"},"application":{"name":"app"},"environment":{"id":"e3","name":"env"}}
      #|]}
    ),
  )
  @debug.assert_eq(
    decode_visible_environment_id(page, "me", "app", "env"),
    Some("e3"),
  )
  @debug.assert_eq(
    decode_visible_environment_id(page, "nobody", "app", "env"),
    None,
  )
  assert_eq(decode_token_account_id(@json.parse("{\"accountId\":\"me\"}")), "me")
  assert_eq(url_component("a b/é~"), "a%20b%2F%C3%A9~")
}

///|
test "primitive round trips" {
  let cases : Array[SchemaValue] = [
    BoolValue(true),
    S8Value(-128),
    S16Value(32767),
    S32Value(-2147483648),
    S64Value(9223372036854775807L),
    U8Value(255),
    U16Value(65535),
    U32Value(4294967295U),
    U64Value(18446744073709551615UL),
    F64Value(3.14),
    StringValue("hello"),
    CharValue(0x1F600),
    PathValue("/tmp/x"),
    UrlValue("https://example.com"),
    DatetimeValue("2024-01-01T00:00:00Z"),
    DurationValue(1000000000L),
  ]
  for c in cases {
    @debug.assert_eq(round_trip(c), c)
  }
}

///|
test "u64 max precision is preserved on the wire" {
  let v = U64Value(18446744073709551615UL)
  let s = schema_value_to_json(v).stringify()
  assert_eq(s, "{\"kind\":\"u64\",\"value\":\"18446744073709551615\"}")
  @debug.assert_eq(round_trip(v), v)
}

///|
test "i64 min precision is preserved on the wire" {
  let v = S64Value(-9223372036854775808L)
  let s = schema_value_to_json(v).stringify()
  assert_eq(s, "{\"kind\":\"s64\",\"value\":\"-9223372036854775808\"}")
}

///|
test "canonical option result binary and exceptional float shapes" {
  assert_eq(
    schema_value_to_json(OptionValue(None)).stringify(),
    "{\"kind\":\"option\",\"value\":{\"inner\":null}}",
  )
  assert_eq(
    schema_value_to_json(ResultValue(ResultOk(None))).stringify(),
    "{\"kind\":\"result\",\"value\":{\"tag\":\"ok\",\"value\":null}}",
  )
  assert_eq(
    schema_value_to_json(BinaryValue(b"\x00\xff", Some("image/png"))).stringify(),
    "{\"kind\":\"binary\",\"value\":{\"bytes\":[0,255],\"mimeType\":\"image/png\"}}",
  )
  assert_eq(
    schema_value_to_json(F64Value(1.0 / 0.0)).stringify(),
    "{\"kind\":\"f64\",\"value\":{\"$float\":\"positive-infinity\"}}",
  )
}

///|
test "native codec rejects noncanonical and loose representations" {
  assert_true(
    raises(() => {
      schema_value_from_json(@json.parse("{\"kind\":\"s64\",\"value\":1}"))
    }),
  )
  assert_true(
    raises(() => {
      schema_value_from_json(@json.parse("{\"kind\":\"u64\",\"value\":\"01\"}"))
    }),
  )
  assert_true(
    raises(() => {
      schema_value_from_json(
        @json.parse("{\"kind\":\"bool\",\"value\":true,\"extra\":0}"),
      )
    }),
  )
  assert_true(
    raises(() => {
      schema_value_from_json(@json.parse("{\"kind\":\"f32\",\"value\":1e39}"))
    }),
  )
  assert_true(
    raises(() => {
      schema_value_from_json(@json.parse("{\"kind\":\"f64\",\"value\":1e400}"))
    }),
  )
  assert_false(
    raises(() => {
      schema_value_from_json(
        @json.parse(
          "{\"kind\":\"f64\",\"value\":{\"$float\":\"positive-infinity\"}}",
        ),
      )
    }),
  )
}

///|
test "REST results validate the returned graph before the local expected schema" {
  let response = @json.parse(
    "{\"agentId\":\"0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d\",\"idempotencyKey\":\"key\",\"result\":{\"graph\":{\"root\":{\"kind\":\"u32\",\"value\":{}}},\"value\":{\"kind\":\"u32\",\"value\":5}}}",
  )
  let decoded = decode_agent_invocation_result(response).result.unwrap()
  assert_eq(decoded, U32Value(5U))
  let expected = public_value_codec(
    "{\"root\":{\"kind\":\"u32\",\"value\":{\"restrictions\":{\"min\":{\"kind\":\"unsigned\",\"value\":10}}}}}",
  )
  // Only the server checks restrictions.
  assert_eq(expected.decode(expected.encode(decoded)), decoded)
}

///|
test "config application JSON preserves reserved-looking fields and nested sums" {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"record\",\"value\":{\"fields\":[{\"name\":\"kind\",\"body\":{\"kind\":\"string\",\"value\":{}}},{\"name\":\"value\",\"body\":{\"kind\":\"option\",\"value\":{\"inner\":{\"kind\":\"result\",\"value\":{\"spec\":{\"ok\":{\"kind\":\"string\",\"value\":{}}}}}}}},{\"name\":\"$option\",\"body\":{\"kind\":\"string\",\"value\":{}}},{\"name\":\"$result\",\"body\":{\"kind\":\"string\",\"value\":{}}}]}}}",
  )
  let value = RecordValue([
    StringValue("application-field"),
    OptionValue(Some(ResultValue(ResultOk(Some(StringValue("nested")))))),
    StringValue("ordinary-option-field"),
    StringValue("ordinary-result-field"),
  ])
  assert_eq(
    codec.encode_application(value),
    @json.parse(
      "{\"$option\":\"ordinary-option-field\",\"$result\":\"ordinary-result-field\",\"kind\":\"application-field\",\"value\":{\"ok\":\"nested\"}}",
    ),
  )
}

///|
test "config application JSON canonicalizes datetime and rejects nested protocol-only values" {
  let datetime = public_value_codec(
    "{\"root\":{\"kind\":\"datetime\",\"value\":{}}}",
  )
  assert_eq(
    datetime
    .encode_application(DatetimeValue("2026-01-01T00:00:00.12Z"))
    .stringify(),
    "\"2026-01-01T00:00:00.120000000Z\"",
  )
  let exceptional = public_value_codec(
    "{\"root\":{\"kind\":\"option\",\"value\":{\"inner\":{\"kind\":\"f64\",\"value\":{}}}}}",
  )
  let value = OptionValue(Some(F64Value(0.0 / 0.0)))
  ignore(exceptional.encode(value))
  assert_true(raises(() => ignore(exceptional.encode_application(value))))
}

///|
test "composite round trips" {
  let cases : Array[SchemaValue] = [
    RecordValue([S32Value(1), StringValue("a"), BoolValue(false)]),
    VariantValue(2, Some(S32Value(42))),
    VariantValue(0, None),
    EnumValue(3),
    FlagsValue([true, false, true]),
    TupleValue([S32Value(1), StringValue("x")]),
    ListValue([S32Value(1), S32Value(2), S32Value(3)]),
    FixedListValue([U8Value(1), U8Value(2)]),
    OptionValue(Some(S32Value(7))),
    OptionValue(None),
    ResultValue(ResultOk(Some(S32Value(1)))),
    ResultValue(ResultErr(Some(StringValue("boom")))),
    ResultValue(ResultOk(None)),
    MapValue([
      { key: StringValue("k"), value: S32Value(1), },
      { key: StringValue("k2"), value: S32Value(2), },
    ]),
    UnionValue("variant-a", S32Value(99)),
    TextValue("bonjour", Some("fr")),
    BinaryValue(b"\x01\x02\x03", Some("application/octet-stream")),
  ]
  for c in cases {
    @debug.assert_eq(round_trip(c), c)
  }
}

///|
test "nested structure round trip" {
  let v = RecordValue([
    ListValue([VariantValue(1, Some(StringValue("hi"))), VariantValue(0, None)]),
    OptionValue(Some(MapValue([{ key: S32Value(1), value: BoolValue(true), }]))),
    ResultValue(
      ResultOk(
        Some(TupleValue([U64Value(18446744073709551615UL), F64Value(2.5)])),
      ),
    ),
  ])
  @debug.assert_eq(round_trip(v), v)
}

///|
test "validation-only traversal visits late nested values without building JSON" {
  let codec = public_value_codec(
    (
      #|{"root":{"kind":"record","value":{"fields":[{"name":"bytes","body":{"kind":"list","value":{"element":{"kind":"u8","value":{}}}}},{"name":"maybe","body":{"kind":"option","value":{"inner":{"kind":"tuple","value":{"elements":[{"kind":"string","value":{}},{"kind":"u16","value":{}}]}}}}}]}}}
    ),
  )
  let value = RecordValue([
    ListValue([U8Value(0), U8Value(251), U8Value(255)]),
    OptionValue(Some(TupleValue([StringValue("árvíz😀"), U16Value(65535)]))),
  ])
  let charge = Ref(0)
  assert_eq(
    codec.encode_at(
      pvc_field(pvc_object(codec.graph), "root"),
      value,
      0,
      charge,
      false,
      materialize=false,
    ),
    Json::null(),
  )
  assert_true(charge.val > 20)
  @debug.assert_eq(codec.decode(codec.encode(value)), value)
  let invalid = RecordValue([
    ListValue([U8Value(0), U8Value(255)]),
    OptionValue(Some(TupleValue([StringValue("ok"), U16Value(65536)]))),
  ])
  assert_true(raises(() => ignore(codec.encode(invalid))))
  assert_true(raises(() => ignore(codec.decode(schema_value_to_json(invalid)))))
}

///|
test "validation-only calls retain stream registration and reset it between calls" {
  let codec = public_value_codec(
    (
      #|{"root":{"kind":"list","value":{"element":{"kind":"stream","value":{"inner":{"kind":"u8","value":{}}}}}}}
    ),
  )
  let value = ListValue([StreamReferenceValue(None, Some("stable-token"))])
  for _ in 0..<2 {
    ignore(codec.encode(value))
    assert_true(codec.stream_elements.contains("stable-token"))
    @debug.assert_eq(codec.decode(schema_value_to_json(value)), value)
    assert_true(codec.stream_elements.contains("stable-token"))
  }
  assert_true(
    raises(() => {
      ignore(
        codec.encode(
          ListValue([
            StreamReferenceValue(None, Some("stable-token")),
            StreamReferenceValue(None, Some("stable-token")),
          ]),
        ),
      )
    }),
  )
  ignore(codec.encode(value))
}

///|
test "schema resolution observes changed definitions and diagnoses aliases afresh" {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"ref\",\"value\":{\"id\":\"n\"}},\"defs\":[{\"id\":\"n\",\"body\":{\"kind\":\"u8\",\"value\":{}}}]}",
  )
  let unrestricted = codec.defs["n"]
  ignore(codec.encode(U8Value(7)))
  codec.defs["n"] = @json.parse("{\"kind\":\"string\",\"value\":{}}")
  assert_true(raises(() => ignore(codec.encode(U8Value(7)))))
  codec.defs["n"] = @json.parse("{\"kind\":\"ref\",\"value\":{\"id\":\"n\"}}")
  assert_true(raises(() => ignore(codec.encode(U8Value(7)))))
  codec.defs["n"] = @json.parse(
    "{\"kind\":\"ref\",\"value\":{\"id\":\"missing\"}}",
  )
  assert_true(raises(() => ignore(codec.encode(U8Value(7)))))
  codec.defs["n"] = unrestricted
  ignore(codec.encode(U8Value(7)))
}

///|
fn encode_create_agent_request(
  parameters : SchemaValue,
  config : Array[AgentConfigEntry],
  env : Array[(String, String)],
  keyed : Bool,
) -> Json raise {
  obj([
    ("parameters", schema_value_to_json(parameters)),
    ("config", Json::array(config.map(encode_config_entry))),
    ("env", obj(env.map(entry => (entry.0, Json::string(entry.1))))),
    ("keyed", Json::boolean(keyed)),
  ])
}

///|
fn encode_agent_invocation_request(
  agent_id : AgentId?,
  parameters : SchemaValue?,
  method_name : String,
  method_parameters : SchemaValue,
  mode : String,
  schedule_at : String?,
  expected_fingerprint : String?,
) -> Json raise {
  let fields : Array[(String, Json)] = [
    ("method", Json::string(method_name)),
    ("methodParameters", schema_value_to_json(method_parameters)),
    ("mode", Json::string(mode)),
  ]
  match agent_id {
    Some(id) => fields.push(("agentId", Json::string(id.to_string())))
    None => ()
  }
  match parameters {
    Some(parameters) =>
      fields.push(("parameters", schema_value_to_json(parameters)))
    None => ()
  }
  match schedule_at {
    Some(at) => fields.push(("scheduleAt", Json::string(at)))
    None => ()
  }
  match expected_fingerprint {
    Some(f) => fields.push(("expectedFingerprint", Json::string(f)))
    None => ()
  }
  obj(fields)
}

///|
test "encoders reject unpaired surrogates and non-scalar chars" {
  fn utf16(units : Array[Int]) -> String {
    String::from_array(units.map(unit => unit.unsafe_to_char()))
  }

  let rejected : Array[SchemaValue] = [
    StringValue(utf16([0xD800])),
    StringValue(utf16([0x61, 0xDC00])),
    StringValue(utf16([0xD83D, 0x61])),
    ListValue([StringValue("ok"), StringValue(utf16([0xDBFF]))]),
    TextValue(utf16([0xDC00]), None),
    PathValue(utf16([0xD800])),
    UrlValue(utf16([0xD800])),
    UnionValue(utf16([0xD800]), BoolValue(true)),
    CharValue(0xD800),
    CharValue(0x110000),
  ]
  for value in rejected {
    let encoders : Array[(SchemaValue) -> Unit raise] = [
      v => ignore(schema_value_render(v)),
      v => ignore(schema_value_to_json(v)),
      v => {
        ignore(render_create_agent_request(v, [], [], false))
      },
      v => {
        ignore(
          render_agent_invocation_request(
            None,
            Some(RecordValue([])),
            "method",
            v,
            "await",
            None,
            None,
          ),
        )
      },
    ]
    for encode in encoders {
      try encode(value) catch {
        BridgeError(message) =>
          assert_true(
            message.contains("unpaired surrogate") ||
            message.contains("Unicode scalar value"),
          )
        other => fail("unexpected error: \{@debug.to_string(other)}")
      } noraise {
        _ => fail("encoded invalid text: \{@debug.to_string(value)}")
      }
    }
  }
  let emoji = utf16([0x61, 0xD83D, 0xDE00])
  assert_eq(round_trip(StringValue(emoji)), StringValue(emoji))
  assert_eq(round_trip(CharValue(0x1F600)), CharValue(0x1F600))
}

///|
fn rejects(f : () -> Unit raise) -> Bool {
  try f() catch {
    _ => true
  } noraise {
    _ => false
  }
}

///|
test "public codec leaves binary MIME restrictions to the server" {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"binary\",\"value\":{\"restrictions\":{\"mimeTypes\":[\"image/png\"]}}}}",
  )
  for value in [
    BinaryValue(b"\x01", None),
    BinaryValue(b"\x01", Some("image/png")),
    BinaryValue(b"\x01", Some("image/gif")),
  ] {
    assert_eq(codec.decode(codec.encode(value)), value)
  }
  let malformed = BinaryValue(b"\x01", Some("not a MIME type"))
  assert_true(rejects(() => ignore(codec.encode(malformed))))
}

///|
test "public codec checks quantity units on encode and decode" {
  let suffixed = public_value_codec(
    "{\"root\":{\"kind\":\"quantity\",\"value\":{\"spec\":{\"baseUnit\":\"kg\",\"allowedSuffixes\":[\"g\",\"kg\"]}}}}",
  )
  let base_only = public_value_codec(
    "{\"root\":{\"kind\":\"quantity\",\"value\":{\"spec\":{\"baseUnit\":\"m\"}}}}",
  )
  for accepted in [
    (suffixed, QuantityValue(5L, 0, "g")),
    (suffixed, QuantityValue(5L, 0, "kg")),
    (base_only, QuantityValue(5L, 0, "m")),
  ] {
    assert_eq(accepted.0.decode(accepted.0.encode(accepted.1)), accepted.1)
  }
  for rejected in [
    (suffixed, QuantityValue(5L, 0, "lb")),
    (base_only, QuantityValue(5L, 0, "km")),
  ] {
    assert_true(rejects(() => ignore(rejected.0.encode(rejected.1))))
    assert_true(
      rejects(() => ignore(rejected.0.decode(schema_value_to_json(rejected.1)))),
    )
  }
}
