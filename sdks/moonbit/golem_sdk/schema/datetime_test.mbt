///|
fn datetime_rejected(text : String) -> Bool {
  try Datetime::parse(text) catch {
    DatetimeError(_) => true
  } noraise {
    _ => false
  }
}

///|
test "datetime prints canonical UTC with up to nine fraction digits" {
  inspect(Datetime::new(0L, 0U), content="1970-01-01T00:00:00Z")
  inspect(
    Datetime::new(1_700_000_000L, 500_000_000U),
    content="2023-11-14T22:13:20.5Z",
  )
  inspect(Datetime::new(-1L, 5U), content="1969-12-31T23:59:59.000000005Z")
  inspect(Datetime::new(-62167219200L, 0U), content="0000-01-01T00:00:00Z")
  inspect(
    Datetime::new(253402300799L, 999_999_999U),
    content="9999-12-31T23:59:59.999999999Z",
  )
}

///|
test "datetime parses any RFC 3339 offset into UTC" {
  assert_eq(
    Datetime::parse("2024-01-01T12:30:00.25+02:00"),
    Datetime::parse("2024-01-01T10:30:00.250Z"),
  )
  inspect(
    Datetime::parse("2023-12-31t23:30:00-01:30"),
    content="2024-01-01T01:00:00Z",
  )
  inspect(
    Datetime::parse("2024-02-29T00:00:00z"),
    content="2024-02-29T00:00:00Z",
  )
  for
    text in [
      "1970-01-01T00:00:00Z", "2023-11-14T22:13:20.5Z", "0000-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z",
    ] {
    assert_eq(Datetime::parse(text).to_string(), text)
  }
}

///|
test "datetime rejects invalid text and years outside 0000..9999" {
  for
    text in [
      "", "2024-01-01", "2024-01-01T00:00:00", "2024-13-01T00:00:00Z", "2023-02-29T00:00:00Z",
      "2024-01-01T24:00:00Z", "2024-01-01T00:00:60Z", "2024-01-01T00:00:00.Z", "2024-01-01T00:00:00.1234567891Z",
      "2024-01-01T00:00:00+24:00", "2024-01-01T00:00:00+0100", "2024-01-01 00:00:00Z",
      "2024-01-01T00:00:00Zx", "0000-01-01T00:00:00+00:01", "9999-12-31T23:59:59-00:01",
    ] {
    assert_true(datetime_rejected(text), msg="accepted \{text}")
  }
  assert_true((try? Datetime::new(253402300800L, 0U)) is Err(_))
  assert_true((try? Datetime::new(0L, 1_000_000_000U)) is Err(_))
}

///|
test "datetime orders by instant and round trips through schema and wire" {
  let early = Datetime::new(-1L, 999_999_999U)
  let late = Datetime::new(0L, 0U)
  assert_true(early < late)
  let value = Datetime::new(1_700_000_000L, 123U)
  assert_eq(from_value_as(to_value_as(value)), value)
  let decoded : Datetime = from_wire(into_wire(value))
  assert_eq(decoded, value)
  let out_of_range : @types.SchemaValueTree = {
    value_nodes: [
      @types.DatetimeValue({ seconds: 253402300800L, nanoseconds: 0U, }),
    ],
    root: 0,
  }
  assert_true((try? (from_wire(out_of_range) : Datetime)) is Err(_))
  assert_true(
    (try? (
      from_value_as(Datetime({ seconds: 0L, nanoseconds: 1_000_000_000U, })) :
      Datetime))
    is Err(_),
  )
}

///|
test "Int16 maps to s16 in schema and wire" {
  let tag : TypeTag[Int16] = type_tag()
  debug_inspect(
    register_in_with(@model.SchemaBuilder::new(), tag).body,
    content="S16(None)",
  )
  for
    value in [
      Int16::from_int(-32768),
      Int16::from_int(0),
      Int16::from_int(32767),
    ] {
    assert_eq((from_value_as(to_value_as(value)) : Int16), value)
    assert_eq((from_wire(into_wire(value)) : Int16), value)
  }
  assert_true((try? (from_value_as(S16(32768)) : Int16)) is Err(_))
  let graph = wire_schema(tag)
  assert_eq(graph.type_nodes[graph.root].body, @types.S16Type(None))
}
