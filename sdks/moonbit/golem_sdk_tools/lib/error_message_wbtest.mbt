///|
test "error_message returns the message of every tool error" {
  let errors : Array[Error] = [
    AgentParseError("agent"),
    ComponentRoleError("role"),
    HttpPathParseError("path"),
    ToolEmitError("emit"),
    ToolMiddlewareEmitError("middleware emit"),
    ToolMiddlewareParseError("middleware parse"),
    ToolParseError("tool"),
  ]
  assert_eq(errors.map(error_message), [
    "agent", "role", "path", "emit", "middleware emit", "middleware parse", "tool",
  ])
}
