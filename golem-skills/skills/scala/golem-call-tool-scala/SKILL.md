---
name: golem-call-tool-scala
description: "Calls a generated typed Golem tool client from Scala. Use when invoking built-in or MCP imported tools, a tool provider, or handling tool errors."
---

# Call a Golem tool from Scala

## Call an existing tool through a generated client

Load `golem-generating-tool-clients` to select a built-in release or MCP import and
add it to the Scala component's `dependencies.tools`. Configure its runtime binding
separately. `golem build` automatically generates the client without an explicit
bridge selection. Include the generated sources in the consumer build.
In the standard sbt template, add this setting to the consumer project's existing
`.settings(...)` block. `LocalRootProject` resolves the application root rather
than the component directory:

```scala
Compile / unmanagedSourceDirectories += (LocalRootProject / baseDirectory).value /
  "golem-temp/bridge-sdk/scala/internal/bash-tool-guest-client/src/main/scala"
```

Call the built-in `bash` release `0.2.1` inside an agent method:

```scala
import golem.bridge.client.bash.BashClient

val pending = BashClient().run("printf 'hello\\n'", "", golem.UInt(600L))
```

`pending` is a `Future` of an `Either`: handle the tool failure or inspect the
result's stdout, stderr, and exit code. Bash returns those strings in its result,
not stream handles. For streaming commands, use the invocation's `collect()` to
drain both channels concurrently and check each outcome.

## Call from a definition owned by the application

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
the tool method instead of `Nothing`. Commands without stdout or stderr, including stdin-only
commands, return `Future[Either[ToolError[E], A]]`; stdin is a `ToolInputStream` parameter. A command
with either output returns `Either[ToolError[E], ToolInvocation[E, A]]`, whose invocation exposes
the result and independent stdout/stderr streams. Drain both outputs concurrently. Handle
`ToolError.Tool` separately from protocol failures. Never edit generated clients.
