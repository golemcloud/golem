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

package golem.host

import golem.{AgentId, Principal, PrincipalRef}
import golem.host.js.{CallerConverter, LegacySnapshotPrincipal}
import zio.test._

import scala.scalajs.js

object CallerConverterSpec extends ZIOSpecDefault {

  private def pair(issuer: String, subject: String): js.Dynamic =
    js.Dynamic.literal("issuer" -> issuer, "subject" -> subject)

  private def uuid(high: Long, low: Long): js.Dynamic =
    js.Dynamic.literal("highBits" -> js.BigInt(high.toString), "lowBits" -> js.BigInt(low.toString))

  private def principal(tag: String, value: js.Dynamic): js.Dynamic =
    js.Dynamic.literal("tag" -> tag, "val" -> value)

  private val user = principal(
    "user",
    js.Dynamic.literal(
      "pair"            -> pair("https://login.example", "alice"),
      "claims"          -> """{"role":"admin"}""",
      "authenticatedAt" -> js.Dynamic.literal("seconds" -> js.BigInt("1700000000"), "nanoseconds" -> 500000000)
    )
  )

  def spec = suite("CallerConverterSpec")(
    suite("callerFromJs")(
      test("reads each of the five kinds of principal") {
        val account = principal(
          "account",
          js.Dynamic.literal(
            "pair"           -> pair("golem", "account:1"),
            "accountId"      -> js.Dynamic.literal("uuid" -> uuid(1L, 2L)),
            "authentication" -> "api-token"
          )
        )
        val agent = principal(
          "agent",
          js.Dynamic.literal(
            "pair"          -> pair("golem", "agent:a"),
            "agentId"       -> js.Dynamic.literal("uuid" -> uuid(3L, 4L)),
            "agentType"     -> "Worker",
            "environmentId" -> js.Dynamic.literal("uuid" -> uuid(5L, 6L))
          )
        )
        val tool = principal(
          "tool",
          js.Dynamic.literal(
            "pair"          -> pair("golem", "tool:t"),
            "environmentId" -> js.Dynamic.literal("uuid" -> uuid(5L, 6L)),
            "toolName"      -> "t"
          )
        )
        val kinds = List(user, account, agent, tool, js.Dynamic.literal("tag" -> "anonymous")).map { p =>
          CallerConverter
            .callerFromJs(js.Dynamic.literal("original" -> p, "immediate" -> p, "tokens" -> js.Array[js.Any]()))
            .immediate
            .kind
            .name
        }
        val decodedAgent = CallerConverter.principalFromJs(agent).asInstanceOf[Principal.Agent]
        assertTrue(
          kinds == List("user", "account", "agent", "tool", "anonymous"),
          decodedAgent.agentId == AgentId(new java.util.UUID(3L, 4L)),
          decodedAgent.agentType == "Worker",
          decodedAgent.environmentId == new java.util.UUID(5L, 6L)
        )
      },
      test("keeps both roles, the token provenance and the user details") {
        val token = js.Dynamic.literal("id" -> uuid(7L, 8L), "maker" -> pair("golem", "account:1"))
        val bare  = js.Dynamic.literal("id" -> uuid(9L, 10L))
        val host  = js.Dynamic.literal(
          "original"  -> user,
          "immediate" -> js.Dynamic.literal("tag" -> "anonymous"),
          "tokens"    -> js.Array[js.Any](token, bare)
        )
        val caller = CallerConverter.callerFromJs(host)
        val alice  = caller.original.asInstanceOf[Principal.User]
        assertTrue(
          caller.immediate == Principal.Anonymous,
          alice.pair == PrincipalRef("https://login.example", "alice"),
          alice.claimsJson == """{"role":"admin"}""",
          alice.authenticatedAt.toEpochMilli == 1700000000500L,
          caller.tokens.map(_.maker) == List(Some(PrincipalRef("golem", "account:1")), None),
          caller.original.toString == """user ("https://login.example","alice")""",
          caller.immediate.toString == "anonymous"
        )
      },
      test("a missing or null tag is anonymous") {
        assertTrue(
          CallerConverter.principalFromJs(js.Dynamic.literal()) == Principal.Anonymous,
          CallerConverter.principalFromJs(js.Dynamic.literal("tag" -> null)) == Principal.Anonymous
        )
      },
      test("reads the u64 halves of a uuid as unsigned bits") {
        val caller = CallerConverter.callerFromJs(
          js.Dynamic.literal(
            "original"  -> js.Dynamic.literal("tag" -> "anonymous"),
            "immediate" -> js.Dynamic.literal("tag" -> "anonymous"),
            "tokens"    -> js.Array[js.Any](
              js.Dynamic.literal(
                "id" -> js.Dynamic.literal("highBits" -> js.BigInt("18446744073709551615"), "lowBits" -> js.BigInt("1"))
              )
            )
          )
        )
        assertTrue(caller.tokens.head.id == new java.util.UUID(-1L, 1L))
      }
    ),
    suite("1.6 snapshot principal")(
      test("accepts each 1.6 form") {
        val forms = List(
          """{"tag":"anonymous"}""",
          """{"tag":"agent","val":{"componentId":"00000000-0000-0000-0000-000000000001","agentId":"a"}}""",
          """{"tag":"golem-user","val":{"accountId":"00000000-0000-0000-0000-000000000002"}}""",
          """{"tag":"oidc","val":{"sub":"s","issuer":"i","claims":"{}","email":null}}"""
        )
        assertTrue(forms.forall(form => LegacySnapshotPrincipal.check(form.getBytes("UTF-8")).isRight))
      },
      test("refuses a malformed principal") {
        val forms = List(
          "not-json",
          """{"tag":"unknown"}""",
          """{"tag":"oidc","val":{"sub":"s"}}""",
          """{"tag":"agent","val":{"componentId":"not-a-uuid","agentId":"a"}}"""
        )
        assertTrue(forms.forall(form => LegacySnapshotPrincipal.check(form.getBytes("UTF-8")).isLeft))
      }
    )
  )
}
