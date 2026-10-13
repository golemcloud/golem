package component_name

import golem.{HostApi, Uuid}
import golem.host.js.{JsAgentId, JsComponentId, JsPromiseId, JsUuid}
import zio.blocks.schema.Schema

import scala.scalajs.js

final case class ApprovalPromiseId(
  componentId: Uuid,
  agentId: String,
  oplogIdx: String
) {
  def toHost: HostApi.PromiseId =
    JsPromiseId(
      JsAgentId(
        JsComponentId(JsUuid(js.BigInt(componentId.highBits.toString), js.BigInt(componentId.lowBits.toString))),
        agentId
      ),
      js.BigInt(oplogIdx)
    )
}

object ApprovalPromiseId {
  implicit val schema: Schema[ApprovalPromiseId] = Schema.derived

  def fromHost(id: HostApi.PromiseId): ApprovalPromiseId =
    ApprovalPromiseId(
      Uuid(BigInt(id.agentId.componentId.uuid.highBits.toString), BigInt(id.agentId.componentId.uuid.lowBits.toString)),
      id.agentId.agentId,
      id.oplogIdx.toString
    )
}
