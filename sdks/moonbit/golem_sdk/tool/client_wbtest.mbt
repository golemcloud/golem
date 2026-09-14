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
