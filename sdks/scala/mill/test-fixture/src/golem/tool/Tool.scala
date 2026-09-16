package golem.tool

import golem.Principal
import golem.schema.{FromSchema, SchemaValue, TypedSchemaValue}
import golem.tool.wire.WitTool

import scala.concurrent.Future

final class ToolInputStream
final class ToolOutputStream
final class ToolMiddlewareInputHandle
final class ToolMiddlewareOutputHandle
final class ToolBuildError
final class ExtendedToolType
final class CanonicalInputModel
final class CanonicalInputValue
final class ToolRpcTransport
final case class ToolInvokeResult(
  result: Option[TypedSchemaValue],
  stdout: Option[ToolOutputStream]
)
final case class ToolMiddlewareResult(
  result: Option[TypedSchemaValue],
  stdout: Option[ToolMiddlewareOutputHandle]
)

sealed trait ToolError[+E]
sealed trait ToolInvokeError[+E]

trait RawToolUnderlying {
  def invoke(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle]
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
}

trait UniversalToolUnderlying extends RawToolUnderlying

final case class UniversalToolMiddlewareInvocation(
  toolName: String,
  toolMetadata: WitTool,
  commandPath: List[String],
  input: TypedSchemaValue,
  stdin: Option[ToolMiddlewareInputHandle],
  principal: Principal
)

trait UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation,
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
}

trait ToolErrorSchema[E] {
  def fromErrorPayloadValue(value: TypedSchemaValue): Either[String, E]
}

object ToolClientRuntime {
  def encodeParams(
    values: => List[(String, SchemaValue)]
  ): Either[ToolError[Nothing], List[(String, SchemaValue)]] = ???

  def countFlagValue(value: Int): SchemaValue = ???

  def staticInputModel(
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String]
  ): Either[String, CanonicalInputModel] = ???

  def buildInputFromModel(
    model: Either[String, CanonicalInputModel],
    values: List[(String, SchemaValue)]
  ): Either[ToolError[Nothing], TypedSchemaValue] = ???

  def runInfallible(
    transport: ToolRpcTransport,
    commandPath: List[String],
    input: Either[ToolError[Nothing], TypedSchemaValue],
    stdin: Option[ToolInputStream]
  ): Future[Either[ToolError[Nothing], ToolInvokeResult]] = ???

  def complete[E, A](
    call: Future[Either[ToolError[E], ToolInvokeResult]]
  )(decode: ToolInvokeResult => Either[ToolError[Nothing], A]): Future[Either[ToolError[E], A]] = ???

  def decodeUnitResult(result: ToolInvokeResult): Either[ToolError[Nothing], Unit] = ???

  def decodeValueResult[A](
    result: ToolInvokeResult,
    from: FromSchema[A]
  ): Either[ToolError[Nothing], A] = ???
}

object ToolUnderlyingRuntime {
  def encodeParams(
    values: => List[(String, SchemaValue)]
  ): Either[ToolInvokeError[Nothing], List[(String, SchemaValue)]] = ???

  def countFlagValue(value: Int): SchemaValue = ???

  def staticInputModel(
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String]
  ): Either[String, CanonicalInputModel] = ???

  def buildInputFromModel(
    model: Either[String, CanonicalInputModel],
    values: List[(String, SchemaValue)]
  ): Either[ToolInvokeError[Nothing], TypedSchemaValue] = ???

  def runInfallible(
    underlying: RawToolUnderlying,
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String],
    input: Either[ToolInvokeError[Nothing], TypedSchemaValue],
    stdin: Option[ToolMiddlewareInputHandle]
  ): Future[Either[ToolInvokeError[Nothing], ToolMiddlewareResult]] = ???

  def complete[E, A](
    call: Future[Either[ToolInvokeError[E], ToolMiddlewareResult]]
  )(decode: ToolMiddlewareResult => Either[ToolError[Nothing], A]): Future[Either[ToolInvokeError[E], A]] = ???

  def decodeUnitResult(result: ToolMiddlewareResult): Either[ToolError[Nothing], Unit] = ???

  def decodeValueResult[A](
    result: ToolMiddlewareResult,
    from: FromSchema[A]
  ): Either[ToolError[Nothing], A] = ???
}
