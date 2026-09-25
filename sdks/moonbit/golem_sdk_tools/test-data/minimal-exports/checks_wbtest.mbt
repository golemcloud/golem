///|
fn run_fixture_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @asyncCore.with_waitableset(() => {
    body()
    @asyncCore.task_returned()
  })
  for _ in 0..<100 {
    if callback == 0 {
      return
    }
    assert_eq(callback & 15, 1)
    callback = @asyncCore.cb(0, 0, 0)
  }
  fail("fixture did not complete synchronously")
}

///|
test "generated exports discover exactly the source definitions" {
  let agents = @agentGuest.discover_agent_types().unwrap()
  let tools = @toolGuest.discover_tools().unwrap()
  assert_eq(agents.length(), if has_agents { 1 } else { 0 })
  assert_eq(tools.length(), if has_tools { 1 } else { 0 })
  assert_eq(@middlewareGuest.discover_tool_middlewares().unwrap().length(), 0)
  assert_true(
    @toolGuest.get_tool("missing")
    is Err(@types.ToolError::InvalidToolName("missing")),
  )
}

///|
test "generated tool invocation and snapshot hooks" {
  run_fixture_async(() => {
    @asyncCore.with_task_group(group => {
      let empty : @types.SchemaValueTree = {
        value_nodes: [@types.RecordValue([])],
        root: 0,
      }
      if has_agents {
        assert_true(
          @agentGuest.initialize(
            "Counter",
            empty,
            @common.Principal::Anonymous,
            group,
          )
          is Ok(_),
        )
        let input : @types.SchemaValueTree = {
          value_nodes: [@types.RecordValue([1]), @types.S32Value(11)],
          root: 0,
        }
        let result = @agentGuest.invoke(
            "add",
            input,
            @common.Principal::Anonymous,
            group,
          )
          .unwrap()
          .unwrap()
        assert_eq(result.value_nodes, [@types.S32Value(18)])
        let snapshot = @saveSnapshot.save(group)
        assert_eq(snapshot.mime_type, "application/json")
        assert_eq(
          Bytes::from_array([ for byte in snapshot.payload => byte ]),
          b"{\"version\":1,\"principal\":{\"tag\":\"anonymous\"},\"state\":{\"value\":18}}",
        )
        assert_true(
          @loadSnapshot.load(snapshot, group)
          is Err("Agent instance is already initialized"),
        )
      } else {
        assert_true(
          @agentGuest.initialize(
            "Counter",
            empty,
            @common.Principal::Anonymous,
            group,
          )
          is Err(@common.AgentError::InvalidType("Counter")),
        )
        assert_true(
          @agentGuest.invoke("add", empty, @common.Principal::Anonymous, group)
          is Err(@common.AgentError::InvalidMethod("add")),
        )
        let snapshot = @saveSnapshot.save(group)
        assert_eq(snapshot.payload.length(), 0)
        assert_true(
          @loadSnapshot.load(snapshot, group)
          is Err("Component defines no agents"),
        )
      }
      let input = @model.typed_schema_value_to_wit({
        graph: {
          defs: [],
          root: @model.schema_type(@model.SchemaTypeBody::Record([])),
        },
        value: @model.SchemaValue::Record([]),
      })
      let result = @toolGuest.invoke(
        "probe",
        ["ping"],
        input,
        None,
        None,
        @common.Principal::Anonymous,
        group,
      )
      if has_tools {
        let result = result.unwrap()
        let output = @modelHost.typed_schema_value_from_wit(
          result.result.unwrap(),
        )
        assert_eq(output.value, @model.SchemaValue::String("pong"))
      } else {
        assert_true(result is Err(@types.ToolError::InvalidToolName("probe")))
      }
    })
  })
}
