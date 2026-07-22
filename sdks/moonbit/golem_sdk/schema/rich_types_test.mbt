///|
struct TestBytesUnit {} derive(Show, Eq)

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
fn[T : IntoSchema + FromSchema] rich_roundtrip(v : T) -> T raise {
  let typed = try_into_typed_schema_value(v)
  let wit = @model.typed_schema_value_to_wit(typed)
  let back = @model.typed_schema_value_from_wit(wit)
  assert_eq(back.graph, typed.graph)
  T::from_value(back.value)
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
    rich_roundtrip(@types.Datetime::{
      seconds: 1_700_000_000L,
      nanoseconds: 123U,
    }),
    @types.Datetime::{ seconds: 1_700_000_000L, nanoseconds: 123U },
  )
  assert_eq(rich_roundtrip(Duration::new(-42L)), Duration::new(-42L))
  assert_eq(
    rich_roundtrip((Quantity::new(1500L, 1, "kb") : Quantity[TestBytesUnit])),
    (Quantity::new(1500L, 1, "kb") : Quantity[TestBytesUnit]),
  )
}

///|
test "rich schema shapes" {
  inspect(
    Path::register_in(@model.SchemaBuilder::new()).body,
    content=(
      #|Path({direction: IN_OUT, kind: ANY, allowed_mime_types: None, allowed_extensions: None})
    ),
  )
  inspect(
    Url::register_in(@model.SchemaBuilder::new()).body,
    content=(
      #|Url({allowed_schemes: None, allowed_hosts: None})
    ),
  )
  let datetime_tag : TypeTag[@types.Datetime] = type_tag()
  inspect(
    register_in_with(@model.SchemaBuilder::new(), datetime_tag).body,
    content=(
      #|Datetime
    ),
  )
  inspect(
    Duration::register_in(@model.SchemaBuilder::new()).body,
    content=(
      #|Duration
    ),
  )
  let quantity_tag : TypeTag[Quantity[TestBytesUnit]] = type_tag()
  inspect(
    register_in_with(@model.SchemaBuilder::new(), quantity_tag).body,
    content=(
      #|Quantity({base_unit: bytes, allowed_suffixes: [kb, mb], min: None, max: None})
    ),
  )
}

///|
test "quantity rejects disallowed unit" {
  let accepted : Quantity[TestBytesUnit] = from_value_as(
    @model.SchemaValue::Quantity({ mantissa: 1L, scale: 0, unit: "mb" }),
  )
  assert_eq(accepted, Quantity::new(1L, 0, "mb"))
  let rejected_base_unit : Result[Quantity[TestBytesUnit], FromSchemaError] = try? from_value_as(
    @model.SchemaValue::Quantity({ mantissa: 1L, scale: 0, unit: "bytes" }),
  )
  inspect(
    rejected_base_unit,
    content=(
      #|Err(Custom(quantity unit 'bytes' is not allowed for bytes))
    ),
  )
  let rejected : Result[Quantity[TestBytesUnit], FromSchemaError] = try? from_value_as(
    @model.SchemaValue::Quantity({ mantissa: 1L, scale: 0, unit: "seconds" }),
  )
  inspect(
    rejected,
    content=(
      #|Err(Custom(quantity unit 'seconds' is not allowed for bytes))
    ),
  )
}

///|
test "quantity constructor rejects disallowed unit" {
  let rejected : Result[Quantity[TestBytesUnit], FromSchemaError] = try? Quantity::new(
    1L, 0, "seconds",
  )
  inspect(
    rejected,
    content=(
      #|Err(Custom(quantity unit 'seconds' is not allowed for bytes))
    ),
  )
}
