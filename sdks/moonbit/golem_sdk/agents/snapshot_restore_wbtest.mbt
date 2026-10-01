///|
priv struct RestoreTestAgent {
  value : Bytes
}

///|
impl RawAgent for RestoreTestAgent with fn invoke(
  _self,
  _method_name,
  _input,
  _principal,
) {
  Ok(None)
}

///|
impl Snapshottable for RestoreTestAgent with fn save_snapshot(self) {
  self.value
}

///|
fn restore_test_agent_type() -> @common.AgentType raise {
  let def : AgentTypeDef = {
    type_name: "RestoreTestAgent",
    kind: REGULAR,
    description: "",
    source_language: "moonbit",
    constructor_: { name: None, description: "", prompt_hint: None, input: [], },
    methods: [],
    dependencies: [],
    mode: DURABLE,
    http_mount: None,
    snapshotting: Disabled,
    config: [],
  }
  def.to_agent_type()
}

///|
#warnings("-unused_async")
test "snapshot restoration installs only after success" {
  let registered = RegisteredAgent::{
    name: "RestoreTestAgent",
    agent_type: restore_test_agent_type(),
    construct: async fn(_input, _principal) {
      Err(@common.AgentError::InvalidInput("constructor must not run"))
    },
    load_snapshot: None,
  }
  let context : SnapshotRestoreContext = {
    agent_id: "RestoreTestAgent(identity)",
    agent_type: "RestoreTestAgent",
    identity_fields: decode_snapshot_identity(
      @schema.into_wire_record(["identity", 42U]),
    ),
    phantom_id: None,
    principal: @common.Principal::Anonymous,
  }
  let identity : String = context.identity(0)
  debug_inspect(identity, content="\"identity\"")
  let numeric_identity : UInt = context.identity(1)
  assert_eq(numeric_identity, 42U)
  let failed = finish_restoration(
    registered,
    @common.Principal::Anonymous,
    Err("restore failed"),
  )
  debug_inspect(failed, content="Err(\"restore failed\")")
  debug_inspect(get_agent_instance() is None, content="true")
  let agent = RestoreTestAgent::{ value: b"restored", }
  let restored = finish_restoration(
    registered,
    context.principal,
    Ok({
      raw: agent,
      snapshottable: Some(agent),
      snapshot_format: Some(Binary),
    }),
  )
  debug_inspect(restored, content="Ok(())")
  guard get_agent_instance() is Some(instance) else {
    fail("restored agent was not installed")
  }
  debug_inspect(instance.principal, content="Anonymous")
  guard instance.snapshottable is Some(saver) else {
    fail("restored agent has no snapshot saver")
  }
  assert_eq(saver.save_snapshot(), b"restored")
}

///|
test "failed snapshot restoration does not commit decoded identity resources" {
  let mut callback = @async-core.with_waitableset(async fn() {
    let mut dropped = 0
    let tree : @types.SchemaValueTree = {
      value_nodes: [
        @types.SecretValue(@types.Secret::Secret(42)),
        @types.RecordValue([0]),
      ],
      root: 1,
    }
    let reader = @schema.WireReader::new(tree, drop_resource=_ => dropped += 1)
    ignore(@schema.wire_node(reader, tree.root))
    let context : SnapshotRestoreContext = {
      agent_id: "FailedRestore()",
      agent_type: "FailedRestore",
      identity_fields: { reader, fields: [0], },
      phantom_id: None,
      principal: @common.Principal::Anonymous,
    }
    register_agent({
      name: "FailedRestore",
      agent_type: restore_test_agent_type(),
      construct: (_input, _principal) => {
        Err(@common.AgentError::InvalidInput("constructor must not run"))
      },
      load_snapshot: Some((_bytes, context) => {
        let _secret : @model.GuestSecretHandle = context.identity(0)
        Err("rejected state")
      }),
    })
    let result = restore_and_set_instance("FailedRestore", b"bad", context)
    assert_eq(result, Err("rejected state"))
    assert_eq(dropped, 1)
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    assert_true(steps < 1000 && (callback & 0xf) == 1)
    callback = @async-core.cb(0, 0, 0)
  }
}
