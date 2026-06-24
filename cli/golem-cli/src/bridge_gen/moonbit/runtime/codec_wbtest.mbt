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
  assert_eq(s, "{\"kind\":\"u64\",\"value\":18446744073709551615}")
  @debug.assert_eq(round_trip(v), v)
}

///|
test "i64 min precision is preserved on the wire" {
  let v = S64Value(-9223372036854775808L)
  let s = schema_value_to_json(v).stringify()
  assert_eq(s, "{\"kind\":\"s64\",\"value\":-9223372036854775808}")
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
      { key: StringValue("k"), value: S32Value(1) },
      { key: StringValue("k2"), value: S32Value(2) },
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
    ListValue([
      VariantValue(1, Some(StringValue("hi"))),
      VariantValue(0, None),
    ]),
    OptionValue(Some(MapValue([{ key: S32Value(1), value: BoolValue(true) }]))),
    ResultValue(ResultOk(Some(TupleValue([U64Value(18446744073709551615UL), F64Value(2.5)])))),
  ])
  @debug.assert_eq(round_trip(v), v)
}
