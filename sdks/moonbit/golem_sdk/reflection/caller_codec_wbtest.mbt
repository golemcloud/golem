///|
test "complete binding accepts the exact name and constructor shape" {
  validate_complete_id(
    "MarketAnalyst",
    {
      type_name: "MarketAnalyst",
      constructor_value: @model.SchemaValue::String("markets"),
      phantom_id: None,
    },
    (@schema.type_tag() : @schema.TypeTag[String]),
  )
}

///|
test "complete binding rejects a different agent name" {
  try
    validate_complete_id(
      "MarketAnalyst",
      {
        type_name: "OtherAgent",
        constructor_value: @model.SchemaValue::String("markets"),
        phantom_id: None,
      },
      (@schema.type_tag() : @schema.TypeTag[String]),
    )
  catch {
    Identity(message) => assert_true(message.contains("OtherAgent"))
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected the mismatched name to be rejected")
  }
}

///|
test "complete binding rejects a different constructor shape" {
  try
    validate_complete_id(
      "MarketAnalyst",
      {
        type_name: "MarketAnalyst",
        constructor_value: @model.SchemaValue::U64(42),
        phantom_id: None,
      },
      (@schema.type_tag() : @schema.TypeTag[String]),
    )
  catch {
    Identity(message) => assert_true(message.contains("constructor"))
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected the mismatched constructor shape to be rejected")
  }
}
