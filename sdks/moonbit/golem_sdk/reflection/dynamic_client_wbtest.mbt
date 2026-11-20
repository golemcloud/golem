// Dynamic client tests use the fake RPC host in scripts/run-wasm-test.mjs.

///|
extern "wasm" fn reset_rpc_host() = "golem:test" "reset-rpc-host"

///|
extern "wasm" fn set_rpc_host_error(tag : Int32) = "golem:test" "set-rpc-host-error"

///|
extern "wasm" fn rpc_host_calls_ffi(ret : Int32) = "golem:test" "rpc-host-calls"

///|
extern "wasm" fn push_rpc_host_buffer(ptr : Int32, size : Int32) = "golem:test" "push-permission-host-buffer"

///|
extern "wasm" fn rpc_guest_malloc(size : Int32) -> Int32 =
  #|(func (param i32) (result i32) (local i32)
  #| local.get 0 i32.const 4 i32.add call $moonbit.gc.malloc
  #| local.tee 1 i32.const 0 call $moonbit.init_array8
  #| local.get 1)

///|
extern "wasm" fn rpc_load32(ptr : Int32) -> Int32 =
  #|(func (param i32) (result i32) local.get 0 i32.load)

///|
extern "wasm" fn rpc_ptr2str(ptr : Int32, len : Int32) -> String =
  #|(func (param i32) (param i32) (result i32)
  #| local.get 0
  #| local.get 1 call $moonbit.init_array16
  #| local.get 0)

///|
fn fresh_rpc_host() -> Unit {
  reset_rpc_host()
  for _ in 0..<16 {
    push_rpc_host_buffer(rpc_guest_malloc(512), 512)
  }
}

///|
fn rpc_host_calls() -> String {
  let ret = rpc_guest_malloc(8)
  rpc_host_calls_ffi(ret)
  rpc_ptr2str(rpc_load32(ret), rpc_load32(ret + 4))
}

///|
test "dynamic create passes keyed, config and env to the host" {
  fresh_rpc_host()
  let config = [@rpc.typed_config_value(["greeting"], "hi")]
  @test_support.run_async(async fn() {
    let client = DynamicAgentClient::create(
      "Counter",
      @model.SchemaValue::Record([String("a")]),
      keyed=true,
      config~,
      env=[("MODE", "test")],
    )
    assert_eq(
      client.agent_id(),
      @schema.AgentId::from_uuid(@schema.Uuid::from_bits(42, 17)),
    )
    client.drop()
  })
  inspect(
    rpc_host_calls(),
    content=(
      #|create Counter nodes=2 keyed=true config=[greeting] env=[MODE=test]
      #|agent-id
    ),
  )
}

///|
test "dynamic keyed create of a current key raises the typed conflict" {
  fresh_rpc_host()
  set_rpc_host_error(5)
  let mut raised = None
  @test_support.run_async(async fn() {
    try
      DynamicAgentClient::create(
        "Counter",
        @model.SchemaValue::Record([String("a")]),
        keyed=true,
      )
    catch {
      error => raised = Some(error)
    } noraise {
      _ => fail("expected the keyed create to fail")
    }
  })
  match raised {
    Some(Remote(Conflict(message))) =>
      inspect(message, content="the key is current")
    other => fail("expected a conflict, got \{Repr(other)}")
  }
}

///|
test "dynamic upsert and get_by_id map to the host functions" {
  fresh_rpc_host()
  let upserted = DynamicAgentClient::upsert(
    "Counter",
    @model.SchemaValue::Record([String("a")]),
  )
  upserted.drop()
  let id = @schema.AgentId::from_uuid(@schema.Uuid::from_bits(42, 17))
  let bound = DynamicAgentClient::get_by_id("Counter", id)
  bound.drop()
  inspect(
    rpc_host_calls(),
    content=(
      #|upsert Counter nodes=2 root=1
      #|get-by-id Counter 00000000-0000-002a-0000-000000000011
    ),
  )
  set_rpc_host_error(2)
  try DynamicAgentClient::get_by_id("Counter", id) catch {
    Remote(NotFound(message)) =>
      inspect(message, content="no agent of the type has the ID")
    error => fail("expected not-found, got \{repr(error)}")
  } noraise {
    _ => fail("expected the lookup to fail")
  }
}
