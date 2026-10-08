package sources.scala.native

import golem.{BaseAgent, ULong}
import golem.bridge.client.native_conformance.NativeConformanceClient
import golem.runtime.annotations.{
  agentDefinition,
  agentImplementation,
  universalToolMiddleware
}
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.tool.{
  RpcError,
  ToolError,
  ToolInvokeError,
  ToolMiddleware,
  ToolMiddlewareResult,
  UniversalToolMiddleware,
  UniversalToolMiddlewareInvocation,
  UniversalToolUnderlying
}

import scala.concurrent.{ExecutionContext, Future}

@agentDefinition()
trait ScalaNativeToolSourceAgent extends BaseAgent {
  class Id(val name: String)
  def exercise(): Future[List[String]]
}

@agentImplementation()
final class ScalaNativeToolSourceAgentImpl(name: String) extends ScalaNativeToolSourceAgent {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def exercise(): Future[List[String]] = {
    val client = NativeConformanceClient()
    val stream = client.finiteStream("payload") match {
      case Left(error)       => Future.successful(s"stream-start-error:$error")
      case Right(invocation) =>
        invocation.collect().map {
          collected =>
            val stdout = collected.stdout.fold(
              error => s"stream-output-error:$error",
              _.fold("none")(_.iterator.map(_.toChar).mkString)
            )
            collected.result.fold(
              error => s"stream-error:$error",
              result => s"stream:$stdout:${result.count.value}"
            )
        }
    }

    for {
      success <- client.structured("alpha", ULong(BigInt(7))).map {
        case Left(error) => s"success-error:$error"
        case Right(evidence) =>
          s"success:${evidence.value}:${evidence.count.value}:${evidence.agentAuthorized}"
      }
      error <- client.supportedError("expected").map {
        case Left(value)     => s"error:$value"
        case Right(evidence) => s"unexpected-error-success:$evidence"
      }
      collected <- stream
      middleware <- client.middleware("input").map {
        case Left(error)  => s"middleware-error:$error"
        case Right(value) => s"middleware:$value"
      }
    } yield List(success, error, collected, middleware)
  }
}

@agentDefinition()
trait UnauthorizedScalaNativeToolSourceAgent extends BaseAgent {
  class Id(val name: String)
  def exercise(): Future[String]
}

@agentImplementation()
final class UnauthorizedScalaNativeToolSourceAgentImpl(name: String)
    extends UnauthorizedScalaNativeToolSourceAgent {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def exercise(): Future[String] =
    NativeConformanceClient().structured("denied", ULong(BigInt(0))).map {
      case Left(ToolError.Rpc(RpcError.Denied(message))) => message
      case Left(error)                                   => s"unexpected-error:$error"
      case Right(_)                                      => "unexpected-success"
    }
}

@universalToolMiddleware(name = "scala-native-source-middleware")
final class ScalaNativeSourceMiddleware extends UniversalToolMiddleware {
  override def invoke(
    invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
    val input =
      if invocation.toolName == "native-conformance" && invocation.commandPath == List("middleware")
      then invocation.input.copy(value = rewrite(invocation.input.value))
      else invocation.input
    underlying.invoke(invocation.commandPath, input, invocation.stdin)
  }

  private def rewrite(value: SchemaValue): SchemaValue = value match {
    case SchemaValue.StringValue(argument) =>
      SchemaValue.StringValue(s"middleware($argument)")
    case SchemaValue.RecordValue(values) => SchemaValue.RecordValue(values.map(rewrite))
    case other                           => other
  }
}
