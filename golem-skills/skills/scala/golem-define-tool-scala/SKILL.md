---
name: golem-define-tool-scala
description: "Defines and implements a typed Golem tool in Scala. Use when creating a tool provider or command schema."
---

# Define a Golem tool in Scala

Annotate the definition trait and implementation class. The build plugin generates registration,
metadata, and typed client sources:

```scala
import golem.runtime.annotations.{toolDefinition, toolImplementation}

@toolDefinition(version = "1.0.0")
trait Echo {
  def echo(value: String): String
}

@toolImplementation()
final class EchoImpl extends Echo {
  override def echo(value: String): String = value
}
```

Use `@arg`, `@command`, `@constraint`, `@result`, and `@annotations` when the command-line surface
needs explicit metadata. Custom values require `zio.blocks.schema.Schema`. Keep
`scalacOptions += "-experimental"` enabled for macro annotations and never edit generated client
or registration sources.
