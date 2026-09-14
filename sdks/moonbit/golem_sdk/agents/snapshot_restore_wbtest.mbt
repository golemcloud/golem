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
    identity_fields: [@model.SchemaValue::String("identity")],
    phantom_id: None,
    principal: @common.Principal::Anonymous,
  }
  let identity : String = context.identity(0)
  debug_inspect(identity, content="\"identity\"")
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
