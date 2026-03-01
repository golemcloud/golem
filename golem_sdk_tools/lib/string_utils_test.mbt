///|
test "to_kebab_case - PascalCase" {
  inspect(@lib.to_kebab_case("Counter"), content="counter")
  inspect(@lib.to_kebab_case("TaskManager"), content="task-manager")
}

///|
test "to_kebab_case - snake_case" {
  inspect(@lib.to_kebab_case("get_value"), content="get-value")
  inspect(@lib.to_kebab_case("add_task"), content="add-task")
}

///|
test "to_kebab_case - already lowercase" {
  inspect(@lib.to_kebab_case("counter"), content="counter")
}

///|
test "to_kebab_case - single word" {
  inspect(@lib.to_kebab_case("hello"), content="hello")
}

///|
test "to_kebab_case - empty string" {
  inspect(@lib.to_kebab_case(""), content="")
}

///|
test "rpc_function_name" {
  inspect(
    @lib.rpc_function_name("Counter", "get_value"),
    content="counter.{get-value}",
  )
  inspect(
    @lib.rpc_function_name("TaskManager", "add_task"),
    content="task-manager.{add-task}",
  )
}
