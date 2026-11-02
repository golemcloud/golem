///|
#warnings("-struct_never_constructed")
struct TestBytesUnit {} derive(Debug, Eq)

///|
impl QuantityUnit for TestBytesUnit with fn type_id() {
  "test.BytesUnit"
}

///|
impl QuantityUnit for TestBytesUnit with fn base_unit() {
  "bytes"
}

///|
impl QuantityUnit for TestBytesUnit with fn allowed_suffixes() {
  ["kb", "mb"]
}

///|
#warnings("-struct_never_constructed")
struct DigestText {} derive(Debug, Eq)

///|
impl TextSpec for DigestText with fn type_id() {
  "test.DigestText"
}

///|
impl TextSpec for DigestText with fn languages() {
  []
}

///|
impl TextSpec for DigestText with fn min_length() {
  Some(8U)
}

///|
impl TextSpec for DigestText with fn max_length() {
  Some(64U)
}

///|
impl TextSpec for DigestText with fn regex() {
  Some("^[a-f0-9]+$")
}

///|
fn[T : IntoSchema + FromSchema] rich_roundtrip(v : T) -> T raise {
  let typed = try_into_typed_schema_value(v)
  let wit = @model.typed_schema_value_to_wit(typed)
  let back = @model.typed_schema_value_from_wit(wit, {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  })
  assert_eq(back.graph, typed.graph)
  FromSchema::from_value(back.value)
}

///|
test "direct rich wire codecs preserve units and reject invalid quantity units" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.UrlValue("https://example.com/a"),
      @types.DurationValue({ nanoseconds: -42L, }),
      @types.QuantityValueNode({ mantissa: 1500L, scale: 1, unit: "kb", }),
      @types.TextValue({ text: "deadbeef", language: None, }),
      @types.TupleValue([0, 1, 2, 3]),
    ],
    root: 4,
  }
  let result : (Url, Duration, Quantity[TestBytesUnit], Text[DigestText]) = from_wire(
    tree,
  )
  assert_eq(result.0, Url::new("https://example.com/a"))
  assert_eq(result.1, Duration::new(-42L))
  assert_eq(result.2, Quantity::new(1500L, 1, "kb"))
  assert_eq(result.3, Text::new("deadbeef"))
  assert_eq(into_wire(result), tree)
  let invalid : @types.SchemaValueTree = {
    value_nodes: [
      @types.QuantityValueNode({ mantissa: 1L, scale: 0, unit: "seconds", }),
    ],
    root: 0,
  }
  try {
    let _ : Quantity[TestBytesUnit] = from_wire(invalid)
  } catch {
    WireError::Invalid(_) => ()
  } noraise {
    _ => fail("accepted a unit outside the declared quantity unit set")
  }
}

///|
test "rich type round-trips" {
  let uuid = Uuid::from_bits(0x0123456789ABCDEFUL, 0xFEDCBA9876543210UL)
  assert_eq(rich_roundtrip(uuid), uuid)
  assert_eq(
    rich_roundtrip(Path::new("/tmp/data.txt")),
    Path::new("/tmp/data.txt"),
  )
  assert_eq(
    rich_roundtrip(Url::new("https://example.com")),
    Url::new("https://example.com"),
  )
  assert_eq(
    rich_roundtrip((Text::new("deadbeef") : Text[DigestText])),
    (Text::new("deadbeef") : Text[DigestText]),
  )
  assert_eq(
    rich_roundtrip(@types.Datetime::{
      seconds: 1_700_000_000L,
      nanoseconds: 123U,
    }),
    @types.Datetime::{ seconds: 1_700_000_000L, nanoseconds: 123U, },
  )
  assert_eq(rich_roundtrip(Duration::new(-42L)), Duration::new(-42L))
  assert_eq(
    rich_roundtrip((Quantity::new(1500L, 1, "kb") : Quantity[TestBytesUnit])),
    (Quantity::new(1500L, 1, "kb") : Quantity[TestBytesUnit]),
  )
}

///|
test "uuid has a first-class schema and value" {
  let tag : TypeTag[Uuid] = type_tag()
  assert_eq(into_schema_graph(tag).root, @model.schema_type(@model.Uuid))
  let uuid = Uuid::from_bits(1UL, 2UL)
  let wit : @types.Uuid = { high_bits: 1UL, low_bits: 2UL, }
  assert_eq(uuid.to_wit(), wit)
  assert_eq(Uuid::from_wit(wit), uuid)
  assert_eq(to_value_as(uuid), @model.SchemaValue::Uuid(wit))
  assert_eq(from_value_as(@model.SchemaValue::Uuid(wit)), uuid)
}

///|
test "uuid parses and prints the hyphenated form" {
  let uuid = Uuid::parse("01234567-89AB-cdef-FEDC-ba9876543210")
  assert_eq(uuid.high_bits(), 0x0123456789ABCDEFUL)
  assert_eq(uuid.low_bits(), 0xFEDCBA9876543210UL)
  inspect(uuid, content="01234567-89ab-cdef-fedc-ba9876543210")
  assert_eq(uuid.to_string(), "01234567-89ab-cdef-fedc-ba9876543210")
  assert_eq(Uuid::parse(uuid.to_string()), uuid)
  inspect(
    Uuid::from_bits(0UL, 0UL),
    content="00000000-0000-0000-0000-000000000000",
  )
  inspect(
    Uuid::from_bits(0xFFFFFFFFFFFFFFFFUL, 0xFFFFFFFFFFFFFFFFUL),
    content="ffffffff-ffff-ffff-ffff-ffffffffffff",
  )
  debug_inspect(uuid, content="Uuid(\"01234567-89ab-cdef-fedc-ba9876543210\")")
}

///|
test "uuid parse rejects malformed input" {
  for
    input in [
      "", "0123456789abcdeffedcba9876543210", "01234567-89ab-cdef-fedc-ba987654321",
      "01234567-89ab-cdef-fedc-ba98765432100", "01234567_89ab-cdef-fedc-ba9876543210",
      "0123456g-89ab-cdef-fedc-ba9876543210", "{01234567-89ab-cdef-fedc-ba98765432}",
    ] {
    try Uuid::parse(input) catch {
      UuidParseError(_) => ()
    } noraise {
      _ => fail("accepted malformed UUID '\{input}'")
    }
  }
}

///|
test "uuid ordering and hashing follow the bits" {
  let a = Uuid::from_bits(1UL, 0xFFFFFFFFFFFFFFFFUL)
  let b = Uuid::from_bits(2UL, 0UL)
  assert_true(a < b)
  assert_eq(a.compare(a), 0)
  let set : @hashset.HashSet[Uuid] = @hashset.HashSet([
    a,
    Uuid::from_bits(1UL, 0xFFFFFFFFFFFFFFFFUL),
    b,
  ])
  assert_eq(set.length(), 2)
}

///|
test "uuid random sets the version and variant bits" {
  let a = Uuid::random()
  let b = Uuid::random()
  assert_true(a != b)
  for uuid in [a, b] {
    assert_eq((uuid.high_bits() >> 12) & 0xFUL, 4UL)
    assert_eq(uuid.low_bits() >> 62, 2UL)
    assert_eq(Uuid::parse(uuid.to_string()), uuid)
  }
}

///|
test "rich schema shapes" {
  debug_inspect(
    Path::register_in(@model.SchemaBuilder::new()).body,
    content=(
      #|Path(
      #|  {
      #|    direction: IN_OUT,
      #|    kind: ANY,
      #|    allowed_mime_types: None,
      #|    allowed_extensions: None,
      #|  },
      #|)
    ),
  )
  debug_inspect(
    Url::register_in(@model.SchemaBuilder::new()).body,
    content=(
      #|Url({ allowed_schemes: None, allowed_hosts: None })
    ),
  )
  let text_tag : TypeTag[Text[DigestText]] = type_tag()
  debug_inspect(
    register_in_with(@model.SchemaBuilder::new(), text_tag).body,
    content=(
      #|Text(
      #|  {
      #|    languages: None,
      #|    min_length: Some(8),
      #|    max_length: Some(64),
      #|    regex: Some("^[a-f0-9]+$"),
      #|  },
      #|)
    ),
  )
  let datetime_tag : TypeTag[@types.Datetime] = type_tag()
  debug_inspect(
    register_in_with(@model.SchemaBuilder::new(), datetime_tag).body,
    content=(
      #|Datetime
    ),
  )
  debug_inspect(
    Duration::register_in(@model.SchemaBuilder::new()).body,
    content=(
      #|Duration
    ),
  )
  let quantity_tag : TypeTag[Quantity[TestBytesUnit]] = type_tag()
  debug_inspect(
    register_in_with(@model.SchemaBuilder::new(), quantity_tag).body,
    content=(
      #|Quantity(
      #|  {
      #|    base_unit: "bytes",
      #|    allowed_suffixes: ["kb", "mb"],
      #|    min: None,
      #|    max: None,
      #|  },
      #|)
    ),
  )
}

///|
test "quantity rejects disallowed unit" {
  let accepted : Quantity[TestBytesUnit] = from_value_as(
    @model.SchemaValue::Quantity({ mantissa: 1L, scale: 0, unit: "mb", }),
  )
  assert_eq(accepted, Quantity::new(1L, 0, "mb"))
  let rejected_base_unit : Result[Quantity[TestBytesUnit], FromSchemaError] = try
    from_value_as(
      @model.SchemaValue::Quantity({ mantissa: 1L, scale: 0, unit: "bytes", }),
    )
    |> Ok
  catch {
    e => Err(e)
  }
  debug_inspect(
    rejected_base_unit,
    content=(
      #|Err(Custom("quantity unit 'bytes' is not allowed for bytes"))
    ),
  )
  let rejected : Result[Quantity[TestBytesUnit], FromSchemaError] = try
    from_value_as(
      @model.SchemaValue::Quantity({ mantissa: 1L, scale: 0, unit: "seconds", }),
    )
    |> Ok
  catch {
    e => Err(e)
  }
  debug_inspect(
    rejected,
    content=(
      #|Err(Custom("quantity unit 'seconds' is not allowed for bytes"))
    ),
  )
}

///|
test "quantity constructor rejects disallowed unit" {
  let rejected : Result[Quantity[TestBytesUnit], FromSchemaError] = try
    Quantity::new(1L, 0, "seconds") |> Ok
  catch {
    e => Err(e)
  }
  debug_inspect(
    rejected,
    content=(
      #|Err(Custom("quantity unit 'seconds' is not allowed for bytes"))
    ),
  )
}
