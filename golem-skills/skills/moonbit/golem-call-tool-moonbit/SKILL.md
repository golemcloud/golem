---
name: golem-call-tool-moonbit
description: "Calls a generated typed Golem tool client from MoonBit. Use when invoking a tool provider and handling typed tool errors."
---

# Call a Golem tool from MoonBit

`#derive.tool` generates `<Tool>Client` in `golem_tool_clients.mbt`:

```moonbit
async fn call_echo() -> Result[String, @tool.ToolError[@tool.NoToolError]] {
  let client = EchoClient::new()
  defer client.drop()
  client.echo("hello")
}
```

Call generated tool methods from an `async fn`; MoonBit has no `await` keyword.
Use `new_for(name)` when targeting another registration name. Generated signatures preserve custom
errors and stream capabilities; handle the returned `@tool.ToolError[E]` rather than assuming every
failure is a domain error. Always call `drop()` when finished, and never edit the generated client.
