///|
test "generated creation overrides project recursive data without changing method schemas" {
  let tree : @component.Node = {
    display_name: "creation-root",
    next: Some({ display_name: "creation-child", next: None, }),
  }
  let overrides = @component.collect_shape_config_overrides(
    {
      max_chars: Some(7),
      tree: Some(tree),
      cleared: Some(None),
      enabled: Some(false),
      values: Some([]),
    },
    [],
  )
  assert_eq(overrides[0].path, ["maxChars"])
  assert_eq(overrides[1].path, ["tree"])
  let graph = overrides[1].value.graph
  guard graph.type_nodes[graph.defs[0].body].body is @types.RecordType(fields) else {
    fail("expected the recursive record definition")
  }
  assert_eq(fields.map(field => field.name), ["displayName", "next"])
  let decoded : @component.Node = @schema.from_wire(overrides[1].value.value)
  assert_eq(decoded.display_name, "creation-root")
  assert_eq(decoded.next.map(node => node.display_name), Some("creation-child"))
  let ordinary = @schema.wire_schema(
    (@schema.type_tag() : @schema.TypeTag[@component.Node]),
  )
  guard ordinary.type_nodes[ordinary.defs[0].body].body
    is @types.RecordType(native_fields) else {
    fail("expected the ordinary recursive record definition")
  }
  assert_eq(native_fields.map(field => field.name), ["display_name", "next"])
}
