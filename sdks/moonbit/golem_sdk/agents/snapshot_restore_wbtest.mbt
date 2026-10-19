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
      multipart_snapshottable: None,
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
    let result = restore_and_set_instance(
      "FailedRestore",
      Binary(b"bad"),
      context,
    )
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

///|
impl MultipartSnapshottable for RestoreTestAgent with fn save_snapshot_parts(
  self,
) {
  Ok(
    MultipartSnapshot::from_state(Json::null(), {
      "index": { bytes: self.value, content_type: "application/octet-stream", },
    }),
  )
}

///|
test "multipart failed restore is retryable skips constructor and installs fresh state" {
  state.agent_instance = None
  defer {
    state.agent_instance = None
  }
  let mut callback = @async-core.with_waitableset(async fn() {
    let mut initialized = 0
    let mut restored = 0
    register_agent({
      name: "MultipartRestore",
      agent_type: restore_test_agent_type(),
      construct: (_input, _principal) => {
        initialized += 1
        Err(@common.AgentError::InvalidInput("constructor must not run"))
      },
      load_snapshot: Some((snapshot, _context) => {
        guard snapshot is Multipart(parts) else {
          return Err("Expected multipart snapshot")
        }
        restored += 1
        let bytes = match
          parts.require_part("index", "application/octet-stream") {
          Ok(bytes) => bytes
          Err(message) => return Err(message)
        }
        let agent = RestoreTestAgent::{ value: bytes, }
        Ok({
          raw: agent,
          snapshottable: None,
          snapshot_format: None,
          multipart_snapshottable: Some(agent),
        })
      }),
    })
    let context = fn() -> SnapshotRestoreContext raise {
      {
        agent_id: "MultipartRestore()",
        agent_type: "MultipartRestore",
        identity_fields: decode_snapshot_identity(@schema.into_wire_record([])),
        phantom_id: None,
        principal: @common.Principal::Anonymous,
      }
    }
    assert_true(
      restore_and_set_instance("MultipartRestore", Binary(b"bad"), context())
      is Err(_),
    )
    assert_eq(restored, 0)
    let missing = MultipartSnapshot::from_state(Json::null(), Map([]))
    assert_true(
      restore_and_set_instance(
        "MultipartRestore",
        Multipart(missing),
        context(),
      )
      is Err(_),
    )
    assert_true(get_agent_instance() is None)
    let bytes = b"\x00\xff\r\n"
    let saved = MultipartSnapshot::from_state(Json::null(), {
      "index": { bytes, content_type: "application/octet-stream", },
    })
    assert_eq(
      restore_and_set_instance("MultipartRestore", Multipart(saved), context()),
      Ok(()),
    )
    assert_eq(initialized, 0)
    assert_eq(restored, 2)
    guard get_agent_instance() is Some(instance) &&
      instance.multipart_snapshottable is Some(saver) else {
      fail("multipart saver not installed")
    }
    assert_eq(saver.save_snapshot_parts(), Ok(saved))
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    assert_true(steps < 1000 && (callback & 0xf) == 1)
    callback = @async-core.cb(0, 0, 0)
  }
}
