package component_name

import golem.HostApi
import golem.host.js.{JsAgentId, JsComponentId, JsPromiseId, JsUuid}
import zio.blocks.schema.Schema

import scala.scalajs.js

// Decimal strings preserve the unsigned 64-bit host fields across agent RPC.
final case class ApprovalPromiseId(
  componentIdHighBits: String,
  componentIdLowBits: String,
  agentId: String,
  oplogIdx: String
) {
  def toHost: HostApi.PromiseId =
    JsPromiseId(
      JsAgentId(JsComponentId(JsUuid(js.BigInt(componentIdHighBits), js.BigInt(componentIdLowBits))), agentId),
      js.BigInt(oplogIdx)
    )
}

object ApprovalPromiseId {
  implicit val schema: Schema[ApprovalPromiseId] = Schema.derived

  def fromHost(id: HostApi.PromiseId): ApprovalPromiseId =
    ApprovalPromiseId(
      id.agentId.componentId.uuid.highBits.toString,
      id.agentId.componentId.uuid.lowBits.toString,
      id.agentId.agentId,
      id.oplogIdx.toString
    )
}
