package roles.middleware

import golem.runtime.annotations.{toolDefinition, toolMiddleware}
import golem.tool.ToolInvokeError

import scala.concurrent.Future

@toolDefinition(name = "scala-role-ping", version = "1.0.0")
trait PingTool {
  def ping(value: String): String
}

@toolMiddleware(name = "scala-role-middleware")
final class PingMiddleware extends PingToolMiddleware {
  def ping(
    underlying: PingToolUnderlying,
    value: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    underlying.ping(value)
}
