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
// Shared host identifier and result JS facades
// ---------------------------------------------------------------------------

@js.native
sealed trait JsUuid extends js.Object {
  def highBits: js.BigInt = js.native
  def lowBits: js.BigInt  = js.native
}

object JsUuid {
  def apply(highBits: js.BigInt, lowBits: js.BigInt): JsUuid =
    js.Dynamic.literal("highBits" -> highBits, "lowBits" -> lowBits).asInstanceOf[JsUuid]
}

@js.native
sealed trait JsComponentId extends js.Object {
  def uuid: JsUuid = js.native
}

object JsComponentId {
  def apply(uuid: JsUuid): JsComponentId =
    js.Dynamic.literal("uuid" -> uuid).asInstanceOf[JsComponentId]
}

@js.native
sealed trait JsAgentId extends js.Object {
  def componentId: JsComponentId = js.native
  def agentId: String            = js.native
}

object JsAgentId {
  def apply(componentId: JsComponentId, agentId: String): JsAgentId =
    js.Dynamic.literal("componentId" -> componentId, "agentId" -> agentId).asInstanceOf[JsAgentId]
}

@js.native
sealed trait JsAccountId extends js.Object {
  def uuid: JsUuid = js.native
}

object JsAccountId {
  def apply(uuid: JsUuid): JsAccountId =
    js.Dynamic.literal("uuid" -> uuid).asInstanceOf[JsAccountId]
}

@js.native
sealed trait JsPromiseId extends js.Object {
  def agentId: JsAgentId  = js.native
  def oplogIdx: js.BigInt = js.native
}

object JsPromiseId {
  def apply(agentId: JsAgentId, oplogIdx: js.BigInt): JsPromiseId =
    js.Dynamic.literal("agentId" -> agentId, "oplogIdx" -> oplogIdx).asInstanceOf[JsPromiseId]
}

// ---------------------------------------------------------------------------
// Result<T, E>
// ---------------------------------------------------------------------------

@js.native
sealed trait JsResult[+T, +E] extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsOk[+T] extends JsResult[T, Nothing] {
  @JSName("val") def value: T = js.native
}

@js.native
sealed trait JsErr[+E] extends JsResult[Nothing, E] {
  @JSName("val") def value: E = js.native
}

object JsResult {
  def ok[T](value: T): JsResult[T, Nothing] =
    JsShape.tagged[JsResult[T, Nothing]]("ok", value.asInstanceOf[js.Any])

  def err[E](value: E): JsResult[Nothing, E] =
    JsShape.tagged[JsResult[Nothing, E]]("err", value.asInstanceOf[js.Any])

  def okOptional[T](value: js.UndefOr[T]): JsResult[js.UndefOr[T], Nothing] =
    JsShape.taggedOptional[JsResult[js.UndefOr[T], Nothing]]("ok", value.map(_.asInstanceOf[js.Any]))

  def errOptional[E](value: js.UndefOr[E]): JsResult[Nothing, js.UndefOr[E]] =
    JsShape.taggedOptional[JsResult[Nothing, js.UndefOr[E]]]("err", value.map(_.asInstanceOf[js.Any]))
}
