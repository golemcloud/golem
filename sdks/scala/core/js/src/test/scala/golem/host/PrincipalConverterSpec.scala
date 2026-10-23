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

import golem.Principal
import golem.host.js.PrincipalConverter
import zio.test._

import scala.scalajs.js

object PrincipalConverterSpec extends ZIOSpecDefault {

  def spec = suite("PrincipalConverterSpec")(
    suite("toJson / fromJson roundtrip")(
      test("Anonymous") {
        val p    = Principal.Anonymous
        val json = PrincipalConverter.toJson(p)
        val back = PrincipalConverter.fromJson(json)
        assertTrue(back == Right(p))
      },
      test("Oidc with all fields") {
        val p = Principal.Oidc(
          sub = "user-123",
          issuer = "https://auth.example.com",
          claims = """{"role":"admin"}""",
          email = Some("user@example.com"),
          name = Some("Alice"),
          emailVerified = Some(true),
          givenName = Some("Alice"),
          familyName = Some("Smith"),
          picture = Some("https://example.com/avatar.png"),
          preferredUsername = Some("alice")
        )
        val json = PrincipalConverter.toJson(p)
        val back = PrincipalConverter.fromJson(json)
        assertTrue(back == Right(p))
      },
      test("Oidc with minimal fields") {
        val p = Principal.Oidc(
          sub = "sub-1",
          issuer = "issuer-1",
          claims = "{}"
        )
        val json = PrincipalConverter.toJson(p)
        val back = PrincipalConverter.fromJson(json)
        assertTrue(back == Right(p))
      },
      test("Agent") {
        val p = Principal.Agent(
          componentId = new java.util.UUID(123456789012345678L, 987654321098765432L),
          agentId = "my-agent-1"
        )
        val json = PrincipalConverter.toJson(p)
        val back = PrincipalConverter.fromJson(json)
        assertTrue(back == Right(p))
      },
      test("GolemUser") {
        val p = Principal.GolemUser(
          accountId = new java.util.UUID(111222333444555666L, 777888999000111222L)
        )
        val json = PrincipalConverter.toJson(p)
        val back = PrincipalConverter.fromJson(json)
        assertTrue(back == Right(p))
      }
    ),
    suite("fromJs")(
      test("Anonymous through fromJs") {
        val jsObj = js.Dynamic.literal("tag" -> "anonymous")
        val back  = PrincipalConverter.fromJs(jsObj)
        assertTrue(back == Principal.Anonymous)
      },
      test("null/undefined tag returns Anonymous") {
        val jsObj = js.Dynamic.literal()
        val back  = PrincipalConverter.fromJs(jsObj)
        assertTrue(back == Principal.Anonymous)
      },
      test("GolemUser through fromJs reads the u64 halves as unsigned UUID bits") {
        val max   = js.BigInt("18446744073709551615")
        val jsObj = js.Dynamic.literal(
          "tag" -> "golem-user",
          "val" -> js.Dynamic.literal(
            "accountId" -> js.Dynamic.literal(
              "uuid" -> js.Dynamic.literal("highBits" -> max, "lowBits" -> js.BigInt(0))
            )
          )
        )
        val back = PrincipalConverter.fromJs(jsObj)
        assertTrue(
          back == Principal.GolemUser(java.util.UUID.fromString("ffffffff-ffff-ffff-0000-000000000000"))
        )
      }
    ),
    suite("fromJson error handling")(
      test("invalid JSON returns Left") {
        val result = PrincipalConverter.fromJson("not-json".getBytes("UTF-8"))
        assertTrue(result.isLeft)
      },
      test("non-canonical UUID text returns Left") {
        val result = PrincipalConverter.fromJson(
          """{"tag":"golem-user","val":{"accountId":"550E8400-E29B-41D4-A716-446655440000"}}""".getBytes("UTF-8")
        )
        assertTrue(result.isLeft)
      }
    )
  )
}
