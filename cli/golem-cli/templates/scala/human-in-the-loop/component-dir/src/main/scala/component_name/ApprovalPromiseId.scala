package component_name

import golem.HostApi
import golem.host.js.{JsAgentId, JsComponentId, JsPromiseId, JsUuid}
import zio.blocks.schema.Schema

import java.util.UUID
import scala.scalajs.js

final case class ApprovalPromiseId(
  componentId: UUID,
  agentId: String,
  oplogIdx: String
) {
  def toHost: HostApi.PromiseId =
    JsPromiseId(
      JsAgentId(JsComponentId(JsUuid.fromUuid(componentId)), agentId),
      js.BigInt(oplogIdx)
    )
}

object ApprovalPromiseId {
  implicit val schema: Schema[ApprovalPromiseId] = Schema.derived

  def fromHost(id: HostApi.PromiseId): ApprovalPromiseId =
    ApprovalPromiseId(
      JsUuid.toUuid(id.agentId.componentId.uuid),
      id.agentId.agentId,
      id.oplogIdx.toString
    )
}
