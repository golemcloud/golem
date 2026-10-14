---
name: golem-call-tool-moonbit
description: "Calls a generated typed Golem tool client from MoonBit. Use when invoking built-in or MCP imported tools, a tool provider, or handling typed tool errors."
---

# Call a Golem tool from MoonBit

## Call an existing tool through a generated client

Load `golem-generating-tool-clients` for built-in releases and MCP imports. Add the
tool to the MoonBit component's `dependencies.tools` and configure its runtime
binding separately. `golem build` automatically generates the client without an
explicit bridge selection. Add the generated module as a local dependency, then import
`"bash-tool-guest-client/client" @bash` in the consumer's `moon.pkg`.
Call the built-in `bash` release `0.2.1` inside an async agent method:

```moonbit
let client = @bash.BashClient::new()
defer client.drop()
match client.run("printf 'hello\\n'", "", 600U) {
  Ok(result) => result.stdout
  Err(_) => "bash failed"
}
```

Handle the actual tool error as appropriate for the agent, and check the script's
exit code in the successful result. Bash returns stdout and stderr as strings.
For output attachments, the generated method starts a `TypedToolInvocation`
synchronously; use its `collect()` helper from async code to drain outputs and
check result, stdout, and stderr outcomes independently.

## Call from a definition owned by the application

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
