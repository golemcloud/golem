---
name: golem-call-tool-scala
description: "Calls a generated typed Golem tool client from Scala. Use when invoking a tool provider and handling tool errors."
---

# Call a Golem tool from Scala

`@toolDefinition` generates `<Tool>Client`. Construct it with the generated factory and call the
typed asynchronous methods:

```scala
import golem.tool.ToolError
import scala.concurrent.Future

val client: EchoClient = EchoClient()
val result: Future[Either[ToolError[Nothing], String]] =
  client.echo("hello")
```

Use the exact generated signature: a command with declared errors uses the error type declared by
the tool method instead of `Nothing`. Commands without stdout, including stdin-only commands,
return `Future[Either[ToolError[E], A]]`; stdin is a `ToolInputStream` parameter. A command with
stdout returns `Either[ToolError[E], ToolInvocation[E, A]]`, whose invocation exposes the result and
stream. Handle `ToolError.Tool` separately from protocol failures. Never edit generated clients.
