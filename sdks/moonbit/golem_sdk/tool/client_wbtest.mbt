///|
test "collect preserves structured-result error precedence over stdout failure" {
  let result : Result[InvocationResult, ToolError[NoToolError]] = Err(
    ToolError::Rpc(RpcError::Cancelled),
  )
  let selected = select_collected_terminals(
    result,
    CollectedStdout::Failed(ToolStreamError::Failed("stdout failed")),
  )
  assert_true(selected is Err(ToolError::Rpc(RpcError::Cancelled)))
}

///|
test "collect reports stdout failure after structured success" {
  try
    select_collected_terminals(
      (
        Ok({ result: None, stdout: None, }) :
        Result[InvocationResult, ToolError[NoToolError]]),
      CollectedStdout::Failed(ToolStreamError::Failed("stdout failed")),
    )
  catch {
    ToolStreamError::Failed("stdout failed") => ()
    _ => fail("expected stdout failure")
  } noraise {
    _ => fail("expected stdout failure")
  }
}

///|
fn assert_remote_tool_error(
  wire : @toolCommon.ToolError,
  expected : RemoteToolError,
) -> Unit raise {
  let result : ToolError[NoToolError] = map_rpc_error(
    @toolHost.RpcError::RemoteToolError(wire),
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
    @toolCommon.ToolError::InvalidToolName("bad name"),
    RemoteToolError::InvalidToolName("bad name"),
  )
  assert_remote_tool_error(
    @toolCommon.ToolError::InvalidCommandPath(["bad"]),
    RemoteToolError::InvalidCommandPath(["bad"]),
  )
  assert_remote_tool_error(
    @toolCommon.ToolError::InvalidInput("input"),
    RemoteToolError::InvalidInput("input"),
  )
  assert_remote_tool_error(
    @toolCommon.ToolError::ConstraintViolation("constraint"),
    RemoteToolError::ConstraintViolation("constraint"),
  )
  assert_remote_tool_error(
    @toolCommon.ToolError::InvalidResult("result"),
    RemoteToolError::InvalidResult("result"),
  )
}
