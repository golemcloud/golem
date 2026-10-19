///|
test "collect preserves every result and output outcome" {
  let result : Result[InvocationResult, ToolError[NoToolError]] = Err(
    ToolError::Rpc(RpcError::Cancelled),
  )
  let collected = select_collected_terminals(
    result,
    Err(ToolStreamError::Failed("stdout failed")),
    Ok(Some(b"stderr")),
  )
  assert_true(collected.result is Err(ToolError::Rpc(RpcError::Cancelled)))
  assert_true(collected.stdout is Err(ToolStreamError::Failed("stdout failed")))
  assert_eq(collected.stderr, Ok(Some(b"stderr")))
}

///|
fn assert_remote_tool_error(
  wire : @types.ToolError,
  expected : RemoteToolError,
) -> Unit raise {
  let result : ToolError[NoToolError] = map_rpc_error(
    @types.ToolRpcError::RemoteToolError(wire),
    fn(_, _) { None },
  )
  match result {
    ToolError::RemoteTool(actual) => assert_eq(actual, expected)
    _ => fail("remote tool error lost its structural variant")
  }
}

///|
test "structural remote tool errors stay distinct from RPC errors" {
  assert_remote_tool_error(
    @types.ToolError::InvalidToolName("bad name"),
    RemoteToolError::InvalidToolName("bad name"),
  )
  assert_remote_tool_error(
    @types.ToolError::InvalidCommandPath(["bad"]),
    RemoteToolError::InvalidCommandPath(["bad"]),
  )
  assert_remote_tool_error(
    @types.ToolError::InvalidInput("input"),
    RemoteToolError::InvalidInput("input"),
  )
  assert_remote_tool_error(
    @types.ToolError::ConstraintViolation("constraint"),
    RemoteToolError::ConstraintViolation("constraint"),
  )
  assert_remote_tool_error(
    @types.ToolError::InvalidResult("result"),
    RemoteToolError::InvalidResult("result"),
  )
}

///|
/// A tool input whose encode fails, carrying `card`.
fn failing_tool_input(
  card : @card.GuestPermissionCardHandle,
  quota : Int,
) -> @model.TypedSchemaValue {
  {
    graph: {
      defs: [],
      root: @model.schema_type(@model.SchemaTypeBody::Record([])),
    },
    value: Record(@test_support.failing_card_input(card, quota)),
  }
}

///|
test "a failed tool input encode keeps argument permission cards" {
  @test_support.reset_resource_drop_counts()
  // The encode fails before the client calls the host, so the RPC handle is
  // never used.
  let client : ToolClient = { rpc: @toolHost.ToolRpc::ToolRpc(0), }
  let no_error = fn(
    _ : String,
    _ : @model.TypedSchemaValue,
  ) -> Result[Unit, String]? {
    None
  }
  let card_a = @card.adopt(@types.PermissionCard::PermissionCard(631))
  let triggered = client.trigger(
    [],
    failing_tool_input(card_a, 632),
    None,
    no_error,
  )
  assert_true(triggered is Err(_))
  let card_b = @card.adopt(@types.PermissionCard::PermissionCard(634))
  @test_support.run_async(async fn() {
    let awaited = client.invoke_and_await(
      [],
      failing_tool_input(card_b, 635),
      None,
      no_error,
    )
    assert_true(awaited is Err(_))
  })
  assert_true(card_a.is_present())
  assert_true(card_b.is_present())
  assert_eq(
    @test_support.resource_drop_count(@test_support.QUOTA_TOKEN_DROPS),
    2,
  )
  assert_eq(
    @test_support.resource_drop_count(@test_support.PERMISSION_CARD_DROPS),
    0,
  )
  ignore(@card.take(card_a))
  ignore(@card.take(card_b))
}
