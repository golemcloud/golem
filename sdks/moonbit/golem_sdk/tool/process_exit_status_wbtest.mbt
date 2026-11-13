///|
test "process exit status is the shared named record" {
  let graph = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[ProcessExitStatus]),
  )
  assert_eq(graph.defs.length(), 1)
  assert_eq(graph.defs[0].id, "golem.tool.ProcessExitStatus")
  debug_inspect(
    graph.defs[0].body.body,
    content=(
      #|Record(
      #|  [
      #|    {
      #|      name: "exit_code",
      #|      body: {
      #|        body: U8(None),
      #|        metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|      },
      #|      metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|    },
      #|  ],
      #|)
    ),
  )
  for code in [b'\x00', b'\x07', b'\xff'] {
    let status = { exit_code: code, }
    assert_eq(@schema.to_value_as(status), Record([U8(code)]))
    assert_eq(
      (@schema.from_value_as(Record([U8(code)])) : ProcessExitStatus),
      status,
    )
    assert_eq(
      (@schema.from_wire(@schema.into_wire(status)) : ProcessExitStatus),
      status,
    )
  }
  let wire = @schema.wire_schema(
    (@schema.type_tag() : @schema.TypeTag[ProcessExitStatus]),
  )
  assert_eq(wire.defs.length(), 1)
  assert_eq(wire.defs[0].id, "golem.tool.ProcessExitStatus")
}
