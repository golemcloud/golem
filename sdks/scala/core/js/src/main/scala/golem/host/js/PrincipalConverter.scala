/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.host.js

import golem.{Principal, Uuid}
import zio.blocks.schema.json.{JsonCodec, JsonCodecDeriver}

import scala.scalajs.js

object PrincipalConverter {

  private val codec: JsonCodec[Principal] =
    Principal.schema.derive(JsonCodecDeriver)

  def fromJs(dynamic: js.Dynamic): Principal = {
    val tag = dynamic.tag
    if (js.isUndefined(tag) || tag == null) Principal.Anonymous
    else
      tag.asInstanceOf[String] match {
        case "oidc" =>
          val p = dynamic.asInstanceOf[JsPrincipalOidc].value
          Principal.Oidc(
            sub = p.sub,
            issuer = p.issuer,
            claims = p.claims,
            email = p.email.toOption,
            name = p.name.toOption,
            emailVerified = p.emailVerified.toOption,
            givenName = p.givenName.toOption,
            familyName = p.familyName.toOption,
            picture = p.picture.toOption,
            preferredUsername = p.preferredUsername.toOption
          )
        case "agent" =>
          val p           = dynamic.asInstanceOf[JsPrincipalAgent].value
          val jsUuid      = p.agentId.componentId.uuid
          val componentId = Uuid(
            highBits = BigInt(jsUuid.highBits.toString),
            lowBits = BigInt(jsUuid.lowBits.toString)
          )
          Principal.Agent(
            componentId = componentId,
            agentId = p.agentId.agentId
          )
        case "golem-user" =>
          val p         = dynamic.asInstanceOf[JsPrincipalGolemUser].value
          val jsUuid    = p.accountId.uuid
          val accountId = Uuid(
            highBits = BigInt(jsUuid.highBits.toString),
            lowBits = BigInt(jsUuid.lowBits.toString)
          )
          Principal.GolemUser(accountId = accountId)
        case "anonymous" =>
          Principal.Anonymous
        case _ =>
          Principal.Anonymous
      }
  }

  def toJson(principal: Principal): Array[Byte] =
    codec.encode(principal)

  def fromJson(bytes: Array[Byte]): Either[String, Principal] =
    codec.decode(bytes).left.map(_.toString)
}
