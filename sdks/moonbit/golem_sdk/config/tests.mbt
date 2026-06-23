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
    (@schema.TypeTag::{  } : @schema.TypeTag[String]),
    ["db", "url"],
  )
  inspect(entries.length(), content="1")
  inspect(entries[0].source, content="LOCAL")
  inspect(
    entries[0].path,
    content=(
      #|[db, url]
    ),
  )
  inspect(
    entries[0].value_type.root,
    content=(
      #|{body: String, metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}
    ),
  )
}

///|
test "collect_entries_of - Secret is a SECRET declaration over the inner type" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{  } : @schema.TypeTag[Secret[String]]),
    ["api", "key"],
  )
  inspect(entries.length(), content="1")
  inspect(entries[0].source, content="SECRET")
  inspect(
    entries[0].value_type.root,
    content=(
      #|{body: String, metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}
    ),
  )
}

///|
test "collect_entries_of - Option field is a LOCAL declaration" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{  } : @schema.TypeTag[Int?]),
    ["timeout"],
  )
  inspect(entries.length(), content="1")
  inspect(entries[0].source, content="LOCAL")
}
