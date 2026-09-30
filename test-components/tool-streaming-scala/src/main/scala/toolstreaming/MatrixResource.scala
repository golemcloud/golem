package toolstreaming

import golem.UInt
import golem.config.Secret
import golem.runtime.annotations.*
import golem.schema.AgentStream
import zio.blocks.schema.Schema

import scala.concurrent.ExecutionContext

final case class MatrixResourceConfig(secret: Secret[String])
object MatrixResourceConfig {
  implicit val schema: Schema[MatrixResourceConfig] = Schema.derived
}

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
