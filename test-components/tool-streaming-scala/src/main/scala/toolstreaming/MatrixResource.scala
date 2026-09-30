package toolstreaming

import golem.UInt
import golem.runtime.annotations.*
import golem.schema.AgentStream
import zio.blocks.schema.Schema

import scala.concurrent.ExecutionContext
import scala.concurrent.Future

@toolDefinition(name = "matrix-resource", version = "1.0.0")
trait MatrixResourceTool {
  def typed(): MatrixTypedTool
}

@toolDefinition(name = "typed", version = "1.0.0")
trait MatrixTypedTool {
  def transform(input: AgentStream[UInt]): AgentStream[UInt]
}

@toolImplementation()
final class MatrixResourceToolImpl extends MatrixResourceTool {
  override def typed(): MatrixTypedTool = new MatrixTypedToolImpl
}

final class MatrixTypedToolImpl extends MatrixTypedTool {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic

  override def transform(input: AgentStream[UInt]): AgentStream[UInt] =
    input.map(value => UInt(value.value * 3L + 1L))
}

final case class MatrixResourceObservation(
  secretFirstProvider: String,
  secretSecondProvider: String,
  secretFirstRevealed: Boolean,
  secretSecondRevealed: Boolean,
  secretPrincipal: String,
  secretOwnerAgentId: String,
  quotaProvider: String,
  quotaReserved: Boolean,
  quotaReturnedUsable: Boolean,
  quotaOriginalConsumed: Boolean,
  quotaPrincipal: String,
  quotaOwnerAgentId: String,
  permissionSupported: Boolean,
  permissionProvider: String,
  permissionSameIdentity: Boolean,
  permissionOriginalConsumed: Boolean,
  permissionPrincipal: String,
  permissionOwnerAgentId: String,
  typedValues: List[UInt]
)
object MatrixResourceObservation {
  implicit val schema: Schema[MatrixResourceObservation] = Schema.derived
}

@agentDefinition()
trait ScalaResourceToolStreamingCaller extends golem.BaseAgent {
  class Id(val name: String)
  def matrix_resource_observation(): Future[MatrixResourceObservation]
}

@agentImplementation()
final class ScalaResourceToolStreamingCallerImpl(name: String) extends ScalaResourceToolStreamingCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def matrix_resource_observation(): Future[MatrixResourceObservation] = {
    val values = Iterator(golem.UInt(2), golem.UInt(5), golem.UInt(9))
    val input = AgentStream.fromPull(() => Future.successful(if (values.hasNext) Some(values.next()) else None))

    MatrixResourceToolClient().typed().transform(input).flatMap {
      case Left(error) => Future.failed(new IllegalStateException(s"matrix-resource typed transform failed: $error"))
      case Right(output) =>
        drainAgentStream(output, Nil).map { typedValues =>
          MatrixResourceObservation(
            secretFirstProvider = "",
            secretSecondProvider = "",
            secretFirstRevealed = false,
            secretSecondRevealed = false,
            secretPrincipal = "",
            secretOwnerAgentId = "",
            quotaProvider = "",
            quotaReserved = false,
            quotaReturnedUsable = false,
            quotaOriginalConsumed = false,
            quotaPrincipal = "",
            quotaOwnerAgentId = "",
            permissionSupported = false,
            permissionProvider = "",
            permissionSameIdentity = false,
            permissionOriginalConsumed = false,
            permissionPrincipal = "",
            permissionOwnerAgentId = "",
            typedValues = typedValues
          )
        }
    }
  }

  private def drainAgentStream(stream: AgentStream[golem.UInt], values: List[golem.UInt]): Future[List[golem.UInt]] =
    stream.pull().flatMap {
      case Some(value) => drainAgentStream(stream, values :+ value)
      case None        => Future.successful(values)
    }
}
