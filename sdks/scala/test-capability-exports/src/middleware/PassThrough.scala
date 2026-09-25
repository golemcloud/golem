package capabilityfixture

import golem.runtime.annotations.universalToolMiddleware
import golem.schema.TypedSchemaValue
import golem.tool.*

import scala.concurrent.Future

@universalToolMiddleware(name = "pass-through")
final class PassThrough extends UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
}
