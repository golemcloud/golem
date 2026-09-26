---
name: golem-tools-middleware-scala
description: "Defines typed or universal Golem tool middleware in Scala. Use for validation, policy, auditing, or adapting tool surfaces."
---

# Tool middleware in Scala

The build plugin generates `<Tool>Middleware` and `<Tool>Underlying` from each tool definition.
Extend the generated middleware trait and annotate the concrete no-argument class. The example
assumes the `Echo` definition from `golem-define-tool-scala`.

```scala
import golem.runtime.annotations.toolMiddleware
import golem.tool.ToolInvokeError
import scala.concurrent.Future

@toolMiddleware(name = "echo-policy")
final class EchoPolicy extends EchoMiddleware {
  def echo(
    underlying: EchoUnderlying,
    value: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    underlying.echo(value).toMiddlewareResult
}
```

For adapters, extend `PresentedMiddleware.Adapter[ExpectedUnderlying]` and translate values and
custom errors. Use `@universalToolMiddleware` with `UniversalToolMiddleware` only for arbitrary tool
metadata and raw `TypedSchemaValue` carriers. The underlying is invocation-scoped; never store or
return it. Forward each stream or permission card at most once.
