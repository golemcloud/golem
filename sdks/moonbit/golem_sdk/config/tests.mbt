// Phase 3d tests for the schema-carrier config layer.
//
// The `load`/`get` paths call the WASM host import `get_config_value` and so
// cannot run in a unit test. The host-free surface is config-declaration
// assembly (`collect_entries_of` → `@agents.AgentConfigDef`), which these tests
// cover: source kind (LOCAL vs SECRET), path threading, and a non-empty schema
// graph for the declared value type.

///|
test "collect_entries_of - primitive String is a LOCAL declaration" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{ } : @schema.TypeTag[String]),
    ["db", "url"],
  )
  debug_inspect(entries.length(), content="1")
  debug_inspect(entries[0].source, content="LOCAL")
  debug_inspect(
    entries[0].path,
    content=(
      #|["db", "url"]
    ),
  )
  debug_inspect(
    entries[0].value_type.root,
    content=(
      #|{
      #|  body: String,
      #|  metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|}
    ),
  )
}

///|
test "collect_entries_of - Secret is a SECRET declaration over the inner type" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{ } : @schema.TypeTag[Secret[String]]),
    ["api", "key"],
  )
  debug_inspect(entries.length(), content="1")
  debug_inspect(entries[0].source, content="SECRET")
  debug_inspect(
    entries[0].value_type.root,
    content=(
      #|{
      #|  body: Secret(
      #|    {
      #|      inner: {
      #|        body: String,
      #|        metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|      },
      #|      category: None,
      #|    },
      #|  ),
      #|  metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|}
    ),
  )
}

///|
test "collect_entries_of - Option field is a LOCAL declaration" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{ } : @schema.TypeTag[Int?]),
    ["timeout"],
  )
  debug_inspect(entries.length(), content="1")
  debug_inspect(entries[0].source, content="LOCAL")
}

///|
test "wire configuration declarations share one type pool and preserve secret wrappers" {
  let encoder = @schema.WireTypeEncoder::new()
  let local_entries = append_wire_entries_with(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[UInt64?]),
    ["limits", "max"],
  )
  let secret = append_wire_entries_with(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Secret[String]]),
    ["api", "key"],
  )
  assert_eq(local_entries[0].source, @common.AgentConfigSource::LOCAL)
  assert_eq(local_entries[0].path, ["limits", "max"])
  assert_eq(local_entries[0].value_type, 1)
  assert_eq(encoder.nodes[0].body, @types.U64Type(None))
  assert_eq(encoder.nodes[1].body, @types.OptionType(0))
  assert_eq(secret[0].source, @common.AgentConfigSource::SECRET)
  assert_eq(secret[0].path, ["api", "key"])
  assert_eq(secret[0].value_type, 3)
  assert_eq(encoder.nodes[2].body, @types.StringType)
  assert_eq(
    encoder.nodes[3].body,
    @types.SecretType({ inner: 2, category: None, }),
  )
}
