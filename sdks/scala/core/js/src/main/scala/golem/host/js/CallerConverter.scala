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

import golem.{AgentId, Caller, Principal, PrincipalRef, TokenProvenance}

import java.time.Instant
import scala.scalajs.js

/**
 * Converts the host's `golem:agent/common@2.0.0` caller into the SDK's
 * [[golem.Caller]].
 */
object CallerConverter {

  def callerFromJs(caller: js.Dynamic): Caller =
    Caller(
      original = principalFromJs(caller.original),
      immediate = principalFromJs(caller.immediate),
      tokens = caller.tokens.asInstanceOf[js.Array[js.Dynamic]].toList.map(tokenFromJs)
    )

  def principalFromJs(dynamic: js.Dynamic): Principal = {
    val tag = dynamic.tag
    if (js.isUndefined(tag) || tag == null) Principal.Anonymous
    else
      tag.asInstanceOf[String] match {
        case "user" =>
          val p = dynamic.selectDynamic("val")
          Principal.User(
            pair = refFromJs(p.pair),
            claimsJson = p.claims.asInstanceOf[String],
            authenticatedAt = instantFromJs(p.authenticatedAt)
          )
        case "account" =>
          val p = dynamic.selectDynamic("val")
          Principal.Account(
            pair = refFromJs(p.pair),
            accountId = uuidFromJs(p.accountId.uuid),
            authentication = p.authentication.asInstanceOf[String]
          )
        case "agent" =>
          val p = dynamic.selectDynamic("val")
          Principal.Agent(
            pair = refFromJs(p.pair),
            agentId = AgentId(uuidFromJs(p.agentId.uuid)),
            agentType = p.agentType.asInstanceOf[String],
            environmentId = uuidFromJs(p.environmentId.uuid)
          )
        case "tool" =>
          val p = dynamic.selectDynamic("val")
          Principal.Tool(
            pair = refFromJs(p.pair),
            environmentId = uuidFromJs(p.environmentId.uuid),
            toolName = p.toolName.asInstanceOf[String]
          )
        case _ =>
          Principal.Anonymous
      }
  }

  private def refFromJs(ref: js.Dynamic): PrincipalRef =
    PrincipalRef(ref.issuer.asInstanceOf[String], ref.subject.asInstanceOf[String])

  private def tokenFromJs(token: js.Dynamic): TokenProvenance =
    TokenProvenance(
      id = uuidFromJs(token.id),
      maker = {
        val maker = token.maker
        if (js.isUndefined(maker) || maker == null) None else Some(refFromJs(maker))
      }
    )

  private def uuidFromJs(uuid: js.Dynamic): java.util.UUID = JsUuid.toUuid(uuid.asInstanceOf[JsUuid])

  private def instantFromJs(datetime: js.Dynamic): Instant =
    Instant.ofEpochSecond(
      datetime.seconds.asInstanceOf[js.BigInt].toString.toLong,
      datetime.nanoseconds.asInstanceOf[Int].toLong
    )
}
