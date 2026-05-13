///|
test "rpc_function_name" {
  debug_inspect(@lib.rpc_function_name("Counter", "get_value"), content=(
    #|"get_value"
  ))
  debug_inspect(@lib.rpc_function_name("TaskManager", "add_task"), content=(
    #|"add_task"
  ))
}
