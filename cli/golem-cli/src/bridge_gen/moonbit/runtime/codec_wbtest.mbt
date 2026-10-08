///|
fn round_trip(v : SchemaValue) -> SchemaValue raise {
  let s = schema_value_to_json(v).stringify()
  schema_value_from_json(@json.parse(s))
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
    "{\"agentId\":{\"componentId\":\"component\",\"agentId\":\"agent\"},\"idempotencyKey\":\"key\",\"result\":{\"graph\":{\"root\":{\"kind\":\"u32\",\"value\":{}}},\"value\":{\"kind\":\"u32\",\"value\":5}}}",
  )
  let decoded = decode_agent_invocation_result(response).result.unwrap()
  assert_eq(decoded, U32Value(5U))
  let expected = public_value_codec(
    "{\"root\":{\"kind\":\"u32\",\"value\":{\"restrictions\":{\"min\":{\"kind\":\"unsigned\",\"value\":10}}}}}",
  )
  assert_true(raises(() => ignore(expected.encode(decoded))))
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
