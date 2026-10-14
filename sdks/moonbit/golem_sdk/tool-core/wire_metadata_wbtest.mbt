///|
struct WireOnly {}

///|
impl @schema.WireSchema for WireOnly with fn append_wire_type(encoder) {
  encoder.named("wire-only", () => {
    let index = encoder.push(@types.StringType)
    encoder.nodes[index] = {
      body: @types.StringType,
      metadata: { ..@schema.empty_wire_metadata(), doc: Some("named text"), },
    }
    index
  })
}

///|
test "wire metadata resolves named types without an IntoSchema implementation" {
  let encoder = @schema.WireTypeEncoder::new()
  let original = wire_tool_type(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[WireOnly]),
  )
  let refined = wire_refine_text(original, Some("[a-z]+"), Some(2), Some(8))
  assert_eq(encoder.defs.length(), 1)
  assert_true(encoder.nodes[original.index].body is @types.RefType(0))
  assert_true(encoder.nodes[encoder.defs[0].body].body is @types.StringType)
  assert_eq(encoder.nodes[refined.index].metadata.doc, Some("named text"))
  let literal = wire_tool_literal(refined, Scalar("blue"))
  assert_true(
    literal.value_nodes[literal.root]
    is @types.TextValue({ text: "blue", language: None, }),
  )
}

///|
test "wire constraint literals select collection elements but defaults keep containers" {
  let encoder = @schema.WireTypeEncoder::new()
  let type_ = wire_tool_type(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Array[UInt64]?]),
  )
  let scalar = wire_value_is_literal(type_, Scalar("18446744073709551615"))
  assert_true(
    scalar.value_nodes[scalar.root] is @types.U64Value(18446744073709551615UL),
  )
  let default = wire_tool_literal(type_, List(["17", "18446744073709551615"]))
  guard default.value_nodes[default.root] is @types.OptionValue(Some(list)) else {
    fail("default must preserve the option wrapper")
  }
  guard default.value_nodes[list] is @types.ListValue([first, second]) else {
    fail("default must preserve the list")
  }
  assert_true(default.value_nodes[first] is @types.U64Value(17UL))
  assert_true(
    default.value_nodes[second] is @types.U64Value(18446744073709551615UL),
  )
  try wire_tool_literal(type_, Scalar("17")) catch {
    LiteralTypeMismatch(_) => ()
    error => fail(repr(error))
  } noraise {
    _ => fail("defaults cannot use the value-is scalar fallback")
  }
}

///|
test "wire numeric restrictions retain exact unsigned bounds and reject overflow" {
  let encoder = @schema.WireTypeEncoder::new()
  let type_ = wire_tool_type(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[UInt64]),
  )
  let refined = wire_refine_numeric(
    type_,
    Some("9007199254740993"),
    Some("18446744073709551615"),
    Some("items"),
  )
  guard encoder.nodes[refined.index].body is @types.U64Type(Some(bounds)) else {
    fail("expected u64 restrictions")
  }
  assert_true(bounds.min is Some(@types.Unsigned(9007199254740993UL)))
  assert_true(bounds.max is Some(@types.Unsigned(18446744073709551615UL)))
  assert_eq(bounds.unit, Some("items"))
  let byte = wire_tool_type(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Byte]),
  )
  try wire_refine_numeric(byte, None, Some("256"), None) catch {
    InvalidNumericBound(_) => ()
    error => fail(repr(error))
  } noraise {
    _ => fail("u8 bounds must fit")
  }
}
