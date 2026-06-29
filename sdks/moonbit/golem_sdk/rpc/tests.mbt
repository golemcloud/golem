// Phase 3c tests for the schema-carrier RPC client.
//
// The `AgentClient` methods delegate to `@agentHost.WasmRpc`, which calls WASM
// host imports and therefore cannot run in a unit test. The only host-free
// public function is `typed_config_value`, which encodes a typed value into a
// `@common.TypedAgentConfigValue` carrying a self-describing
// `@types.TypedSchemaValue` (schema graph + value tree). These tests exercise
// that conversion pipeline (`@schema.try_into_typed_schema_value` +
// `@model.typed_schema_value_to_wit`) over the new schema carriers.

///|
test "typed_config_value preserves path and encodes a string value" {
  let cfg = typed_config_value(["section", "name"], "hello")
  inspect(
    cfg.path,
    content=(
      #|[section, name]
    ),
  )
  inspect(cfg.value.value.root, content="0")
  inspect(
    cfg.value.value.value_nodes,
    content=(
      #|[StringValue(hello)]
    ),
  )
  // The carried value is self-describing: a non-empty schema graph must
  // accompany the value tree (guards against a "value only, missing schema"
  // regression in the encode pipeline).
  inspect(cfg.value.graph.type_nodes.length() > 0, content="true")
}

///|
test "typed_config_value encodes an integer value" {
  let cfg = typed_config_value(["count"], 42)
  inspect(
    cfg.path,
    content=(
      #|[count]
    ),
  )
  inspect(cfg.value.value.value_nodes, content="[S32Value(42)]")
}

///|
test "typed_config_value encodes a list value" {
  let cfg = typed_config_value(["tags"], ["a", "b"])
  inspect(
    cfg.path,
    content=(
      #|[tags]
    ),
  )
  inspect(
    cfg.value.value.value_nodes,
    content=(
      #|[StringValue(a), StringValue(b), ListValue([0, 1])]
    ),
  )
}
