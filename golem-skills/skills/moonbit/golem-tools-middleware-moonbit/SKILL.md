---
name: golem-tools-middleware-moonbit
description: "Defines typed or universal Golem tool middleware in MoonBit. Use for validation, policy, auditing, or tool adapters."
---

# Tool middleware in MoonBit

For typed middleware, annotate an empty struct and implement async methods whose first parameter is
the generated underlying. `Echo` must be a same-package `#derive.tool` declaration; see
`golem-define-tool-moonbit`.

```moonbit
#derive.tool_middleware("echo-policy", presented="Echo")
struct EchoPolicy {}

pub async fn EchoPolicy::echo(
  underlying : EchoUnderlying,
  value : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.echo(value)
}
```

Set `expected="OtherTool"` for an adapter and translate its values and errors. Use
`#derive.universal_tool_middleware` on an async free function only when arbitrary tool metadata and
raw carriers are required. MoonBit async functions use no `await` keyword. The underlying tool is
valid only during this middleware invocation and may be called zero, one, or multiple times. Raw
input/result carriers and owned stream or permission-card handles are take-once: do not reuse a
transferred carrier or let invocation-scoped resources escape. Golem checks each underlying call against the calling agent's tool permissions, using the command and arguments the next layer receives, so an adapter that changes them needs the agent to hold grants for both forms.
