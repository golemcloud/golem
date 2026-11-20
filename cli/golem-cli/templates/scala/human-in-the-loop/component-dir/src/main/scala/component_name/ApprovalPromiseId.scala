package component_name

import golem.HostApi
import golem.host.js.{JsAgentId, JsPromiseId, JsUuid}
import zio.blocks.schema.Schema

import java.util.UUID
import scala.scalajs.js

final case class ApprovalPromiseId(
  agentId: UUID,
  fingerprint: UUID,
  oplogIdx: String
) {
  def toHost: HostApi.PromiseId =
    JsPromiseId(
      JsAgentId(JsUuid.fromUuid(agentId)),
      JsUuid.fromUuid(fingerprint),
      js.BigInt(oplogIdx)
    )
}

object ApprovalPromiseId {
  implicit val schema: Schema[ApprovalPromiseId] = Schema.derived

  def fromHost(id: HostApi.PromiseId): ApprovalPromiseId =
    ApprovalPromiseId(
      JsUuid.toUuid(id.agentId.uuid),
      JsUuid.toUuid(id.fingerprint),
      id.oplogIdx.toString
    )
}
