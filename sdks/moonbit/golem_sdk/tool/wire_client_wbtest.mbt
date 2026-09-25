///|
test "concrete client decoding does not consult a supplied result graph" {
  let payload : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 731, },
    value: @schema.into_wire(
      (Some([Ok(17U), Err("no")]) : Array[Result[UInt, String]]?),
    ),
  }
  let result : Result[Array[Result[UInt, String]]?, ToolError[NoToolError]] = decode_wire_result_value({
      result: Some(payload),
      stdout: None,
    },
  )
  guard result is Ok(Some([Ok(17U), Err("no")])) else {
    fail("concrete decoder must consume the wire nodes, not the graph")
  }
}

///|
test "concrete client rejects mismatched values and out-of-bounds children" {
  let graph : @types.SchemaGraph = { type_nodes: [], defs: [], root: 0, }
  let wrong : Result[UInt, ToolError[NoToolError]] = decode_wire_result_value({
    result: Some({
      graph,
      value: { value_nodes: [@types.StringValue("17")], root: 0, },
    }),
    stdout: None,
  })
  assert_true(wrong is Err(Rpc(Protocol(_))))
  let invalid_child : Result[Array[UInt], ToolError[NoToolError]] = decode_wire_result_value({
      result: Some({
        graph,
        value: { value_nodes: [@types.ListValue([19])], root: 0, },
      }),
      stdout: None,
    },
  )
  assert_true(invalid_child is Err(Rpc(Protocol(_))))
}

///|
test "unknown wire tool errors preserve their payload without schema construction" {
  let payload : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 63, },
    value: { value_nodes: [@types.StringValue("opaque")], root: 0, },
  }
  let mapped : ToolError[NoToolError] = map_wire_rpc_error(
    @types.RemoteToolError(
      @types.CustomError({ name: "future-error", payload, }),
    ),
    (_, _) => None,
  )
  guard mapped is UnknownToolError(name, actual) else {
    fail("unknown errors must stay on the wire")
  }
  assert_eq(name, "future-error")
  assert_eq(actual, payload)
}

///|
test "text input refinement follows option and list fields without changing other strings" {
  let value = @schema.into_wire_record([Some(["blue", "green"]), "untouched"])
  wire_text_input_field(value, 0)
  guard value.value_nodes[value.root] is @types.RecordValue([first, second]) &&
    value.value_nodes[first] is @types.OptionValue(Some(list)) &&
    value.value_nodes[list] is @types.ListValue([blue, green]) else {
    fail("input shape changed")
  }
  assert_true(
    value.value_nodes[blue]
    is @types.TextValue({ text: "blue", language: None, }),
  )
  assert_true(
    value.value_nodes[green]
    is @types.TextValue({ text: "green", language: None, }),
  )
  assert_true(value.value_nodes[second] is @types.StringValue("untouched"))
}
