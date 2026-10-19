/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:permissions@0.1.0  –  JS facade traits
//
// The permission-card capability is the opaque `permission-card` resource from
// `golem:core/types@2.0.0`. These facades are internal to the SDK: guest code
// uses the typed `golem.host.PermissionsApi` surface and never sees a raw
// handle.
// ---------------------------------------------------------------------------

@js.native
private[golem] sealed trait JsPermissionCardResource extends js.Object

@js.native
private[golem] sealed trait JsPersistentCard extends js.Object {
  def tag: String                                   = js.native
  @JSName("val") def card: JsPermissionCardResource = js.native
}

@js.native
private[golem] sealed trait JsPermissionTimestamp extends js.Object {
  def seconds: js.BigInt  = js.native
  def nanoseconds: Double = js.native
}

@js.native
private[golem] sealed trait JsPermissionDuration extends js.Object {
  @JSName("iso8601") def iso8601: String = js.native
}

private[golem] object JsPermissionDuration {
  def apply(iso8601: String): JsPermissionDuration =
    js.Dynamic.literal("iso8601" -> iso8601).asInstanceOf[JsPermissionDuration]
}

@js.native
private[golem] sealed trait JsPatternGrant extends js.Object {
  @JSName("class_") def className: String = js.native
  def owner: String                       = js.native
  def recipient: String                   = js.native
  def verb: String                        = js.native
  def resourceId: String                  = js.native
}

private[golem] object JsPatternGrant {
  def apply(className: String, owner: String, recipient: String, verb: String, resourceId: String): JsPatternGrant =
    js.Dynamic
      .literal(
        "class_"     -> className,
        "owner"      -> owner,
        "recipient"  -> recipient,
        "verb"       -> verb,
        "resourceId" -> resourceId
      )
      .asInstanceOf[JsPatternGrant]
}

@js.native
private[golem] sealed trait JsCardView extends js.Object {
  def lowerPositive: js.Array[JsPatternGrant] = js.native
  def lowerNegative: js.Array[JsPatternGrant] = js.native
  def upperPositive: js.Array[JsPatternGrant] = js.native
  def upperNegative: js.Array[JsPatternGrant] = js.native
}

@js.native
private[golem] sealed trait JsKernelModule extends js.Object {
  def className: String       = js.native
  def verbs: js.Array[String] = js.native
}

// --- wallet-card variant: { tag: "concrete" | "template" | "scope", val } ---

@js.native
private[golem] sealed trait JsWalletCard extends js.Object {
  def tag: String                                   = js.native
  @JSName("val") def card: JsPermissionCardResource = js.native
}

// --- holder variant: { tag: "account" | "app" | "agent", val } ---

@js.native
private[golem] sealed trait JsHolder extends js.Object {
  def tag: String = js.native
}

private[golem] object JsHolder {
  def account(uuid: JsUuid): JsHolder     = JsShape.tagged[JsHolder]("account", uuid)
  def app(uuid: JsUuid): JsHolder         = JsShape.tagged[JsHolder]("app", uuid)
  def agent(agentId: JsAgentId): JsHolder = JsShape.tagged[JsHolder]("agent", agentId)
}
