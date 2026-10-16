///|
test "durable function names preserve the legacy empty-interface form" {
  assert_eq(durable_function_name("", "generate"), "generate")
  assert_eq(durable_function_name("golem-ai", "generate"), "golem-ai::generate")
}

///|
test "durable function types map to raw wrapped function types" {
  assert_eq(
    DurableFunctionType::ReadLocal.to_raw(),
    @oplog.WrappedFunctionType::ReadLocal,
  )
  assert_eq(
    DurableFunctionType::WriteRemote.to_raw(),
    @oplog.WrappedFunctionType::WriteRemote,
  )
  assert_eq(
    DurableFunctionType::WriteLocal.to_raw(),
    @oplog.WrappedFunctionType::WriteLocal,
  )
  assert_eq(
    DurableFunctionType::ReadRemote.to_raw(),
    @oplog.WrappedFunctionType::ReadRemote,
  )
}
