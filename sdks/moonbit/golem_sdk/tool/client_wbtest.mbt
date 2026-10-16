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
