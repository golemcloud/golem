///|
struct CollisionText {}

///|
impl @schema.WireSchema for CollisionText with fn append_wire_type(encoder) {
  encoder.named("collision", () => encoder.push(@types.StringType))
}

///|
struct CollisionBool {}

///|
impl @schema.WireSchema for CollisionBool with fn append_wire_type(encoder) {
  encoder.named("collision", () => encoder.push(@types.BoolType))
}

///|
test "panic flat descriptor rejects conflicting named definitions" {
  let encoder = @schema.WireTypeEncoder::new()
  ignore(
    wire_tool_type(
      encoder,
      (@schema.type_tag() : @schema.TypeTag[CollisionText]),
    ),
  )
  ignore(
    wire_tool_type(
      encoder,
      (@schema.type_tag() : @schema.TypeTag[CollisionBool]),
    ),
  )
}

///|
test "flat descriptor reuses equal recursive named bodies with different node indices" {
  let encoder = @schema.WireTypeEncoder::new()
  let build = () => {
    encoder.named("recursive", () => {
      let reference = encoder.named("recursive", () => {
        abort("recursive body expanded twice")
      })
      let next = encoder.push(@types.OptionType(reference))
      encoder.push(
        @types.RecordType([
          { name: "next", body: next, metadata: @schema.empty_wire_metadata(), },
          {
            name: "value",
            body: encoder.push(@types.S64Type(None)),
            metadata: @schema.empty_wire_metadata(),
          },
        ]),
      )
    })
  }
  let first = build()
  let second = build()
  assert_eq(encoder.defs.length(), 1)
  assert_eq(encoder.nodes[first], encoder.nodes[second])
}
