///|
priv struct LoadTestAgent {
  agent_id : String
  agent_type : String
  name : String
  count : UInt32
  state : Bytes
}

///|
impl @agents.RawAgent for LoadTestAgent with fn invoke(
  _self,
  _method_name,
  _input,
  _principal,
) {
  Ok(None)
}

///|
impl @agents.Snapshottable for LoadTestAgent with fn save_snapshot(self) {
  self.state
}

///|
let load_test_agent_id = "0000002a-0000-8011-8000-00000000001d"

///|
let load_test_env : Array[(String, String)] = [
  ("GOLEM_AGENT_ID", load_test_agent_id),
  ("GOLEM_AGENT_TYPE", "LoadTestAgent"),
]

///|
fn register_load_test_agent(restored : Array[LoadTestAgent]) -> Unit raise {
  let def : @agents.AgentTypeDef = {
    type_name: "LoadTestAgent",
    kind: REGULAR,
    description: "",
    source_language: "moonbit",
    constructor_: {
      name: None,
      description: "",
      prompt_hint: None,
      input: [
        {
          name: "name",
          source: UserSupplied(
            @schema.schema_graph_of_tag(
              (@schema.type_tag() : @schema.TypeTag[String]),
            ),
          ),
        },
        { name: "principal", source: AutoInjected(PRINCIPAL), },
        {
          name: "count",
          source: UserSupplied(
            @schema.schema_graph_of_tag(
              (@schema.type_tag() : @schema.TypeTag[UInt32]),
            ),
          ),
        },
      ],
    },
    methods: [],
    dependencies: [],
    mode: DURABLE,
    http_mount: None,
    snapshotting: Disabled,
    config: [],
  }
  @agents.register_agent({
    name: "LoadTestAgent",
    agent_type: def.to_agent_type(),
    construct: (_input, _principal) => {
      Err(@common.AgentError::InvalidInput("constructor must not run"))
    },
    load_snapshot: Some((snapshot, context) => {
      guard snapshot is Binary(state) else {
        return Err("Expected binary snapshot")
      }
      let name : String = context.identity(0) catch { e => return Err(repr(e)) }
      let count : UInt32 = context.identity(1) catch {
        e => return Err(repr(e))
      }
      let agent = LoadTestAgent::{
        agent_id: context.agent_id.to_string(),
        agent_type: context.agent_type,
        name,
        count,
        state,
      }
      restored.push(agent)
      Ok({
        raw: agent,
        snapshottable: Some(agent),
        snapshot_format: Some(Binary),
        multipart_snapshottable: None,
      })
    }),
  })
}

///|
test "load hands the constructor parameters to the restore code" {
  let restored : Array[LoadTestAgent] = []
  register_load_test_agent(restored)
  let snapshot = binary_snapshot([b'\x01', b's', b't'])
  @test_support.run_async(async fn() {
    let wrong_arity = load_with_environment(
      snapshot,
      @schema.into_wire_record(["restored-name", 7U, 8U]),
      load_test_env,
    )
    guard wrong_arity is Err(message) &&
      message.contains("expected 2 field(s), got 3") else {
      fail(
        "unexpected result for a wrong constructor arity: \{repr(wrong_arity)}",
      )
    }
    assert_true(@agents.get_agent_instance() is None)
    assert_eq(restored.length(), 0)
    let result = load_with_environment(
      snapshot,
      @schema.into_wire_record(["restored-name", 7U]),
      load_test_env,
    )
    assert_eq(result, Ok(()))
  })
  assert_eq(restored.length(), 1)
  assert_eq(restored[0].agent_id, load_test_agent_id)
  assert_eq(restored[0].agent_type, "LoadTestAgent")
  assert_eq(restored[0].name, "restored-name")
  assert_eq(restored[0].count, 7U)
  assert_eq(restored[0].state, b"st")
  guard @agents.get_agent_instance() is Some(instance) &&
    instance.snapshottable is Some(saver) else {
    fail("restored agent was not installed")
  }
  assert_eq(saver.save_snapshot(), b"st")
}

///|
test "load drops the constructor capabilities when it fails early" {
  @test_support.reset_resource_drop_counts()
  let constructor_ : @types.SchemaValueTree = {
    value_nodes: [
      @types.SecretValue(@types.Secret::Secret(42)),
      @types.RecordValue([0]),
    ],
    root: 1,
  }
  @test_support.run_async(async fn() {
    let result = load_with_environment(
      binary_snapshot([b'\x01']),
      constructor_,
      [],
    )
    assert_true(result is Err(_))
  })
  assert_eq(@test_support.resource_drop_count(@test_support.SECRET_DROPS), 1)
}
