package sources.scala

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation, universalToolMiddleware}
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.tool.{
  ToolInvokeError,
  ToolMiddleware,
  ToolMiddlewareResult,
  UniversalToolMiddleware,
  UniversalToolMiddlewareInvocation,
  UniversalToolUnderlying
}
import golem.bridge.client.catalog_lookup.CatalogLookupClient

import scala.concurrent.{ExecutionContext, Future}

@agentDefinition()
trait ScalaToolSourceAgent extends BaseAgent {
  class Id(val name: String)
  def invoke(itemId: String): Future[String]
}

@agentImplementation()
final class ScalaToolSourceAgentImpl(name: String) extends ScalaToolSourceAgent {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def invoke(itemId: String): Future[String] =
    CatalogLookupClient().catalogLookup(None, itemId) match {
      case Left(error)       => Future.successful(s"start-error:$error")
      case Right(invocation) =>
        invocation.collect().map {
          collected =>
            val stdout = collected.stdout.fold(error => s"error:$error", _.fold("none")(bytes =>
              bytes.map(byte => f"${byte & 0xff}%02x").mkString
            ))
            val stderr = collected.stderr.fold(error => s"error:$error", _.fold("none")(bytes =>
              bytes.map(byte => f"${byte & 0xff}%02x").mkString
            ))
            collected.result.fold(
              error => s"error:$error",
              result => s"result=$result;stdout=$stdout;stderr=$stderr"
            )
        }
    }
}

@universalToolMiddleware(name = "scala-source-middleware")
final class ScalaSourceMiddleware extends UniversalToolMiddleware {
  override def invoke(
    invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    underlying.invoke(
      invocation.commandPath,
      invocation.input.copy(value = rewrite(invocation.input.value)),
      invocation.stdin
    )

  private def rewrite(value: SchemaValue): SchemaValue = value match {
    case SchemaValue.StringValue("middleware") => SchemaValue.StringValue("middleware-applied")
    case SchemaValue.RecordValue(values)        => SchemaValue.RecordValue(values.map(rewrite))
    case SchemaValue.OptionValue(value)         => SchemaValue.OptionValue(value.map(rewrite))
    case other                                  => other
  }
}
