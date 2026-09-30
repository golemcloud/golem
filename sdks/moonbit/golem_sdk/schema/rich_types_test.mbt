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
