// RPC client tests use the fake host in scripts/run-wasm-test.mjs.
// Configuration tests exercise the schema conversion without host imports.

///|
extern "wasm" fn reset_rpc_host() = "golem:test" "reset-rpc-host"

///|
extern "wasm" fn set_rpc_host_error(tag : Int32) = "golem:test" "set-rpc-host-error"

///|
extern "wasm" fn set_rpc_host_unknown_self_type() = "golem:test" "set-rpc-host-unknown-self-type"

///|
extern "wasm" fn rpc_host_drops() -> Int32 = "golem:test" "rpc-host-drops"

///|
extern "wasm" fn rpc_host_calls_ffi(ret : Int32) = "golem:test" "rpc-host-calls"

///|
extern "wasm" fn push_host_buffer(ptr : Int32, size : Int32) = "golem:test" "push-permission-host-buffer"

///|
fn guest_malloc(size : Int32) -> Int32 {
  @gen.mbt_ffi_cabi_realloc(0, 0, 1, size)
}

///|
extern "wasm" fn load32(ptr : Int32) -> Int32 =
  #|(func (param i32) (result i32) local.get 0 i32.load)

///|
extern "wasm" fn load8(ptr : Int32) -> Int32 =
  #|(func (param i32) (result i32) local.get 0 i32.load8_u)

///|
/// The `rpc-error` cases of the fake host.
const DENIED : Int32 = 1

///|
const NOT_FOUND : Int32 = 2

///|
const CONFLICT : Int32 = 5

///|
let host_agent_id : @schema.AgentId = @schema.AgentId::from_uuid(
  @schema.Uuid::from_bits(42UL, 17UL),
)

///|
fn fresh_rpc_host() -> Unit {
  reset_rpc_host()
  for _ in 0..<16 {
    push_host_buffer(guest_malloc(512), 512)
  }
}

///|
/// The calls that reached the fake host since the last reset, one per line.
fn rpc_host_calls() -> String {
  let ret = guest_malloc(8)
  rpc_host_calls_ffi(ret)
  let ptr = load32(ret)
  let len = load32(ret + 4) * 2
  let text = try! @utf16.decode(
    Bytes::makei(len, i => load8(ptr + i).to_byte()),
    ignore_bom=false,
  )
  ignore(@gen.mbt_ffi_cabi_realloc(ptr, len, 2, 0))
  ignore(@gen.mbt_ffi_cabi_realloc(ret, 8, 4, 0))
  text
}

///|
fn counter_input() -> @types.SchemaValueTree {
  { value_nodes: [StringValue("a"), RecordValue([0])], root: 1, }
}

///|
test "upsert connects through the host upsert" {
  fresh_rpc_host()
  let client = AgentClient::upsert("Counter", counter_input())
  assert_eq(client.agent_type_name, "Counter")
  inspect(rpc_host_calls(), content="upsert Counter nodes=2 root=1")
  client.drop()
  assert_eq(rpc_host_drops(), 1)
}

///|
test "upsert raises the typed host error" {
  fresh_rpc_host()
  set_rpc_host_error(DENIED)
  try AgentClient::upsert("Counter", counter_input()) catch {
    @agentHost.RpcError::Denied(message) =>
      inspect(message, content="denied by the host")
    error => fail("expected denied, got \{Repr(error)}")
  } noraise {
    _ => fail("expected the upsert to fail")
  }
  assert_eq(rpc_host_drops(), 0)
}

///|
test "get_by_id passes the typed agent ID to the host" {
  fresh_rpc_host()
  let client = AgentClient::get_by_id("Counter", host_agent_id)
  inspect(
    rpc_host_calls(),
    content="get-by-id Counter 00000000-0000-002a-0000-000000000011",
  )
  client.drop()
}

///|
test "get_by_id of an unknown or wrong-type ID raises not-found" {
  fresh_rpc_host()
  set_rpc_host_error(NOT_FOUND)
  try AgentClient::get_by_id("Counter", host_agent_id) catch {
    @agentHost.RpcError::NotFound(message) =>
      inspect(message, content="no agent of the type has the ID")
    error => fail("expected not-found, got \{Repr(error)}")
  } noraise {
    _ => fail("expected the lookup to fail")
  }
  assert_eq(rpc_host_drops(), 0)
}

///|
test "create without options makes an agent without a key" {
  fresh_rpc_host()
  @test_support.run_async(async fn() {
    let client = AgentClient::create("Counter", counter_input())
    client.drop()
  })
  inspect(
    rpc_host_calls(),
    content="create Counter nodes=2 keyed=false config=[] env=[]",
  )
  assert_eq(rpc_host_drops(), 1)
}

///|
test "create passes keyed, config and env to the host" {
  fresh_rpc_host()
  let config = [
    typed_config_value(["limits", "max"], 3),
    typed_config_value(["name"], "x"),
  ]
  @test_support.run_async(async fn() {
    let client = AgentClient::create(
      "Counter",
      counter_input(),
      keyed=true,
      config~,
      env=[("MODE", "test"), ("REGION", "eu")],
    )
    client.drop()
  })
  inspect(
    rpc_host_calls(),
    content="create Counter nodes=2 keyed=true config=[limits.max,name] env=[MODE=test,REGION=eu]",
  )
}

///|
test "keyed create of a current key raises the typed conflict" {
  fresh_rpc_host()
  set_rpc_host_error(CONFLICT)
  let mut raised = None
  @test_support.run_async(async fn() {
    try AgentClient::create("Counter", counter_input(), keyed=true) catch {
      error => raised = Some(error)
    } noraise {
      _ => fail("expected the keyed create to fail")
    }
  })
  match raised {
    Some(@agentHost.RpcError::Conflict(message)) =>
      inspect(message, content="the key is current")
    other => fail("expected conflict, got \{Repr(other)}")
  }
  assert_eq(rpc_host_drops(), 0)
}

///|
test "agent_id calls the host agent-id method and returns the typed ID" {
  fresh_rpc_host()
  let client = AgentClient::upsert("Counter", counter_input())
  let mut id = None
  @test_support.run_async(async fn() { id = Some(client.agent_id()) })
  assert_eq(id, Some(host_agent_id))
  inspect(
    rpc_host_calls(),
    content=(
      #|upsert Counter nodes=2 root=1
      #|agent-id
    ),
  )
  client.drop()
}

///|
test "self binding checks the type of the bound ID without creation" {
  fresh_rpc_host()
  let client = AgentClient::bind_self("Counter")
  assert_eq(client.agent_type_name, "Counter")
  inspect(
    rpc_host_calls(),
    content=(
      #|bind-self
      #|get-agent-type-by-agent-id 00000000-0000-002a-0000-000000000011
    ),
  )
  assert_eq(rpc_host_drops(), 0)
  client.drop()
  assert_eq(rpc_host_drops(), 1)
}

///|
test "self binding refuses wrong types and releases the resource" {
  fresh_rpc_host()
  try AgentClient::bind_self("Other") catch {
    @common.AgentError::InvalidType(message) =>
      assert_eq(message, "self binding expects Other, identity is Counter")
    _ => fail("expected invalid-type")
  } noraise {
    _ => fail("expected refusal")
  }
  assert_eq(rpc_host_drops(), 1)
}

///|
test "self binding propagates an unknown type and host refusal without leaking" {
  fresh_rpc_host()
  set_rpc_host_unknown_self_type()
  try AgentClient::bind_self("Counter") catch {
    @common.AgentError::InvalidAgentId(message) =>
      assert_eq(
        message, "self binding returned an agent ID of no deployed agent type: 00000000-0000-002a-0000-000000000011",
      )
    _ => fail("expected invalid-agent-id")
  } noraise {
    _ => fail("expected refusal")
  }
  assert_eq(rpc_host_drops(), 1)
  fresh_rpc_host()
  set_rpc_host_error(DENIED)
  try AgentClient::bind_self("Counter") catch {
    @agentHost.RpcError::Denied(message) =>
      assert_eq(message, "denied by the host")
    _ => fail("expected denied")
  } noraise {
    _ => fail("expected refusal")
  }
  assert_eq(rpc_host_drops(), 0)
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
