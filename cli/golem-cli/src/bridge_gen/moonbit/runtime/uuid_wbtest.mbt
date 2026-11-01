///|
let sample_uuid_text : String = "dd00721b-3329-4621-a01d-c71f02cd78c6"

///|
fn sample_uuid() -> Uuid {
  Uuid::from_bits(0xdd00721b33294621UL, 0xa01dc71f02cd78c6UL)
}

///|
fn uuid_codec(root : String) -> PublicValueCodec raise {
  public_value_codec("{\"root\":\{root}}")
}

///|
let uuid_schema : String = "{\"kind\":\"uuid\",\"value\":{}}"

///|
fn expect_bridge_error(f : () -> Unit raise) -> Unit raise {
  try f() catch {
    BridgeError(_) => ()
    other => fail("unexpected error: \{other}")
  } noraise {
    _ => fail("expected a BridgeError")
  }
}

///|
test "uuid parses and prints the hyphenated form" {
  let uuid = Uuid::parse("DD00721B-3329-4621-a01d-C71F02CD78C6")
  assert_eq(uuid, sample_uuid())
  assert_eq(uuid.high_bits(), 0xdd00721b33294621UL)
  assert_eq(uuid.low_bits(), 0xa01dc71f02cd78c6UL)
  assert_eq(uuid.to_string(), sample_uuid_text)
  inspect(uuid, content=sample_uuid_text)
  assert_eq(Uuid::parse(uuid.to_string()), uuid)
  inspect(
    Uuid::from_bits(0UL, 0UL),
    content="00000000-0000-0000-0000-000000000000",
  )
  assert_eq(Uuid::parse_canonical(sample_uuid_text), uuid)
}

///|
test "uuid parse rejects malformed input" {
  for
    input in [
      "", "dd00721b33294621a01dc71f02cd78c6", "not-a-uuid", "dd00721b-3329-4621-a01d-c71f02cd78c",
      "dd00721b-3329-4621-a01d-c71f02cd78c66", "dd00721b_3329-4621-a01d-c71f02cd78c6",
      "dd00721g-3329-4621-a01d-c71f02cd78c6",
    ] {
    expect_bridge_error(() => ignore(Uuid::parse(input)))
  }
  expect_bridge_error(() => {
    ignore(Uuid::parse_canonical("DD00721B-3329-4621-A01D-C71F02CD78C6"))
  })
}

///|
test "uuid ordering, hashing and randomness" {
  let a = Uuid::from_bits(1UL, 0xFFFFFFFFFFFFFFFFUL)
  let b = Uuid::from_bits(2UL, 0UL)
  assert_true(a < b)
  let seen : Map[Uuid, Int] = Map([])
  seen[a] = 1
  seen[Uuid::from_bits(1UL, 0xFFFFFFFFFFFFFFFFUL)] = 2
  seen[b] = 3
  assert_eq(seen.length(), 2)
  let r1 = Uuid::random()
  let r2 = Uuid::random()
  assert_true(r1 != r2)
  for r in [r1, r2] {
    assert_eq((r.high_bits() >> 12) & 0xFUL, 4UL)
    assert_eq(r.low_bits() >> 62, 2UL)
    assert_eq(Uuid::parse_canonical(r.to_string()), r)
  }
}

///|
test "public value codec encodes and decodes a top-level uuid" {
  let codec = uuid_codec(uuid_schema)
  let wire = "{\"kind\":\"uuid\",\"value\":\"\{sample_uuid_text}\"}"
  assert_eq(codec.encode(UuidValue(sample_uuid())).stringify(), wire)
  assert_eq(
    codec.encode_application(UuidValue(sample_uuid())).stringify(),
    "\"\{sample_uuid_text}\"",
  )
  assert_eq(codec.decode(@json.parse(wire)), UuidValue(sample_uuid()))
  assert_eq(schema_value_render(UuidValue(sample_uuid())), wire)
}

///|
test "public value codec handles nested uuids" {
  let option_codec = uuid_codec(
    "{\"kind\":\"option\",\"value\":{\"inner\":\{uuid_schema}}}",
  )
  let some = OptionValue(Some(UuidValue(sample_uuid())))
  assert_eq(option_codec.decode(option_codec.encode(some)), some)
  assert_eq(
    option_codec.encode_application(some).stringify(),
    "\"\{sample_uuid_text}\"",
  )
  let none = OptionValue(None)
  assert_eq(option_codec.decode(option_codec.encode(none)), none)
  let list_codec = uuid_codec(
    "{\"kind\":\"list\",\"value\":{\"element\":\{uuid_schema}}}",
  )
  let list = ListValue([
    UuidValue(sample_uuid()),
    UuidValue(Uuid::from_bits(0UL, 1UL)),
  ])
  assert_eq(list_codec.decode(list_codec.encode(list)), list)
  assert_eq(
    list_codec.encode_application(list).stringify(),
    "[\"\{sample_uuid_text}\",\"00000000-0000-0000-0000-000000000001\"]",
  )
  let record_codec = uuid_codec(
    "{\"kind\":\"record\",\"value\":{\"fields\":[{\"name\":\"id\",\"body\":\{uuid_schema}},{\"name\":\"name\",\"body\":{\"kind\":\"string\",\"value\":{}}}]}}",
  )
  let record = RecordValue([UuidValue(sample_uuid()), StringValue("x")])
  assert_eq(record_codec.decode(record_codec.encode(record)), record)
  assert_eq(
    record_codec.encode_application(record).stringify(),
    "{\"id\":\"\{sample_uuid_text}\",\"name\":\"x\"}",
  )
}

///|
test "public value codec rejects non-canonical and mismatched uuids" {
  let codec = uuid_codec(uuid_schema)
  for
    value in [
      "DD00721B-3329-4621-A01D-C71F02CD78C6", "dd00721b33294621a01dc71f02cd78c6",
      "not-a-uuid",
    ] {
    expect_bridge_error(() => {
      ignore(
        codec.decode(
          Json::object({
            "kind": Json::string("uuid"),
            "value": Json::string(value),
          }),
        ),
      )
    })
  }
  expect_bridge_error(() => ignore(codec.encode(StringValue(sample_uuid_text))))
  expect_bridge_error(() => {
    ignore(
      codec.decode(
        @json.parse("{\"kind\":\"string\",\"value\":\"\{sample_uuid_text}\"}"),
      ),
    )
  })
}
