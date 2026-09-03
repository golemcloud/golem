package fixture

import golem.runtime.annotations.{toolDefinition, toolMiddleware, universalToolMiddleware}
import golem.schema._
import golem.tool.{
  ToolInvokeError,
  ToolMiddlewareResult,
  UniversalToolMiddleware,
  UniversalToolMiddlewareInvocation,
  UniversalToolUnderlying
}

import scala.concurrent.Future
import scala.util._

@toolDefinition(name = "presented", version = "1.0.0")
trait Presented {
  def call(value: String): String
  def inspect(value: Try[String]): Try[String]
}

@toolDefinition(name = "backend", version = "1.0.0")
trait Backend {
  def execute(value: String): String
}

@toolMiddleware(name = "transparent")
final class Transparent extends PresentedMiddleware {
  def call(
    underlying: PresentedUnderlying,
    value: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    underlying.call(value)

  def inspect(
    underlying: PresentedUnderlying,
    value: Try[String]
  ): Future[Either[ToolInvokeError[Nothing], Try[String]]] =
    underlying.inspect(value)
}

@toolMiddleware(name = "adapter")
final class Adapter extends PresentedMiddleware.Adapter[BackendUnderlying] {
  def call(
    underlying: BackendUnderlying,
    value: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    underlying.execute(value)

  def inspect(
    underlying: BackendUnderlying,
    value: Try[String]
  ): Future[Either[ToolInvokeError[Nothing], Try[String]]] =
    Future.successful(Right(value))
}

@universalToolMiddleware(name = "universal")
final class Universal extends UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation,
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
}
