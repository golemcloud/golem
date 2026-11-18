// RPC identity tests use the fake host in scripts/run-wasm-test.mjs.
// Configuration tests exercise the schema conversion without host imports.

///|
extern "wasm" fn reset_self_binding_host(mode : Int32) = "golem:test" "reset-self-binding-host"

///|
extern "wasm" fn self_binding_drops() -> Int32 = "golem:test" "self-binding-drops"

///|
extern "wasm" fn push_host_buffer(ptr : Int32, size : Int32) = "golem:test" "push-permission-host-buffer"

///|
extern "wasm" fn guest_malloc(size : Int32) -> Int32 =
  #|(func (param i32) (result i32) (local i32)
  #| local.get 0 i32.const 4 i32.add call $moonbit.gc.malloc
  #| local.tee 1 i32.const 0 call $moonbit.init_array8
  #| local.get 1)

///|
fn fresh_binding_host(mode : Int32) -> Unit {
  reset_self_binding_host(mode)
  for _ in 0..<8 {
    push_host_buffer(guest_malloc(256), 256)
  }
}

///|
test "self binding preserves the recorded phantom identity without creation" {
  fresh_binding_host(0)
  let client = AgentClient::bind_self("Counter")
  assert_eq(
    client.get_agent_id(),
    "Counter(\"recorded\")[00000000-0000-0011-0000-00000000001d]",
  )
  assert_eq(
    client.phantom_id(),
    Some(@schema.Uuid::parse("00000000-0000-0011-0000-00000000001d")),
  )
  assert_eq(self_binding_drops(), 0)
  client.drop()
  assert_eq(self_binding_drops(), 1)
}

///|
test "self binding refuses wrong types and releases the resource" {
  fresh_binding_host(0)
  try AgentClient::bind_self("Other") catch {
    @common.AgentError::InvalidType(message) =>
      assert_eq(message, "self binding expects Other, identity is Counter")
    _ => fail("expected invalid-type")
  } noraise {
    _ => fail("expected refusal")
  }
  assert_eq(self_binding_drops(), 1)
}

///|
test "self binding propagates parse and host refusal without leaking" {
  fresh_binding_host(1)
  try AgentClient::bind_self("Counter") catch {
    @common.AgentError::InvalidAgentId(message) => assert_eq(message, "bad id")
    _ => fail("expected invalid-agent-id")
  } noraise {
    _ => fail("expected refusal")
  }
  assert_eq(self_binding_drops(), 1)
  fresh_binding_host(2)
  try AgentClient::bind_self("Counter") catch {
    @agentHost.RpcError::Denied(message) =>
      assert_eq(message, "ephemeral owner")
    _ => fail("expected denied")
  } noraise {
    _ => fail("expected refusal")
  }
  assert_eq(self_binding_drops(), 0)
}

///|
test "typed_config_value preserves path and encodes a string value" {
  let cfg = typed_config_value(["section", "name"], "hello")
  debug_inspect(
    cfg.path,
    content=(
      #|["section", "name"]
    ),
  )
  debug_inspect(cfg.value.value.root, content="0")
  debug_inspect(
    cfg.value.value.value_nodes,
    content=(
      #|[StringValue("hello")]
    ),
  )
  // The carried value is self-describing: a non-empty schema graph must
  // accompany the value tree (guards against a "value only, missing schema"
  // regression in the encode pipeline).
  debug_inspect(cfg.value.graph.type_nodes.length() > 0, content="true")
}

///|
test "typed_config_value encodes an integer value" {
  let cfg = typed_config_value(["count"], 42)
  debug_inspect(
    cfg.path,
    content=(
      #|["count"]
    ),
  )
  debug_inspect(cfg.value.value.value_nodes, content="[S32Value(42)]")
}

///|
test "typed_config_value encodes a list value" {
  let cfg = typed_config_value(["tags"], ["a", "b"])
  debug_inspect(
    cfg.path,
    content=(
      #|["tags"]
    ),
  )
  debug_inspect(
    cfg.value.value.value_nodes,
    content=(
      #|[StringValue("a"), StringValue("b"), ListValue([0, 1])]
    ),
  )
}
