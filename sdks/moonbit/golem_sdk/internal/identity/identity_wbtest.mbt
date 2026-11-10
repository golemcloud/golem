///|
test "model and wire constructors prepare the same identity" {
  let value = @model.SchemaValue::Record([String("counter")])
  let wire = @model.schema_value_to_wit(value)
  for phantom_id in [None, Some(@schema.Uuid::from_bits(42UL, 17UL))] {
    let packed = Constructor::from_value("Counter", value, phantom_id)
    let raw = Constructor::from_wire("Counter", wire, phantom_id)
    assert_eq(packed.type_name, "Counter")
    assert_eq(packed.type_name, raw.type_name)
    assert_eq(packed.input, raw.input)
    assert_eq(packed.phantom_id, phantom_id.map(@schema.Uuid::to_wit))
    assert_eq(packed.phantom_id, raw.phantom_id)
  }
}

///|
test "parsed constructor decoding retains schema errors" {
  let input = @model.typed_schema_value_to_wit({
    graph: {
      defs: [],
      root: @model.schema_type(@model.SchemaTypeBody::String),
    },
    value: String("counter"),
  })
  let parsed : Parsed = { type_name: "Counter", input, phantom_id: None, }
  assert_true(parsed.constructor_value() is String("counter"))
  let malformed : Parsed = {
    ..parsed,
    input: { ..input, value: { root: 1, value_nodes: [], }, },
  }
  try malformed.constructor_value() catch {
    @model.SchemaModelError::DecodeError(_) => ()
    error => fail("expected a schema decode error, got \{repr(error)}")
  } noraise {
    _ => fail("expected a constructor schema decode error")
  }
}
