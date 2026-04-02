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

package golem.runtime

import golem.{Principal, Uuid}
import golem.host.js.PrincipalConverter
import zio.blocks.schema.json.Json
import zio.test._

object SnapshotEnvelopeSpec extends ZIOSpecDefault {

  private val testPrincipal = Principal.Oidc(
    sub = "user-42",
    issuer = "https://auth.example.com",
    claims = """{"role":"admin"}""",
    email = Some("user@example.com")
  )

  private val testAgentPrincipal = Principal.Agent(
    componentId = Uuid(BigInt("123456789012345678"), BigInt("987654321098765432")),
    agentId = "agent-1"
  )

  private def buildJsonEnvelope(principal: Principal, stateJson: String): Array[Byte] = {
    val principalJsonStr = new String(PrincipalConverter.toJson(principal), "UTF-8")
    val envelope         = s"""{"version":1,"principal":$principalJsonStr,"state":$stateJson}"""
    envelope.getBytes("UTF-8")
  }

  private def parseJsonEnvelope(bytes: Array[Byte]): (Principal, String) =
    Json.parse(bytes) match {
      case Right(envelope) =>
        val p = envelope
          .get("principal")
          .one
          .toOption
          .flatMap(pJson => PrincipalConverter.fromJson(pJson.printBytes).toOption)
          .getOrElse(Principal.Anonymous)
        val stateStr = envelope
          .get("state")
          .one
          .toOption
          .map(_.print)
          .getOrElse("{}")
        (p, stateStr)
      case Left(err) =>
        throw new RuntimeException(s"Failed to parse envelope: $err")
    }

  private def buildBinaryEnvelopeV2(principal: Principal, stateBytes: Array[Byte]): Array[Byte] = {
    val principalBytes = PrincipalConverter.toJson(principal)
    val totalLength    = 1 + 4 + principalBytes.length + stateBytes.length
    val fullSnapshot   = new Array[Byte](totalLength)
    fullSnapshot(0) = 2.toByte
    fullSnapshot(1) = ((principalBytes.length >>> 24) & 0xff).toByte
    fullSnapshot(2) = ((principalBytes.length >>> 16) & 0xff).toByte
    fullSnapshot(3) = ((principalBytes.length >>> 8) & 0xff).toByte
    fullSnapshot(4) = (principalBytes.length & 0xff).toByte
    System.arraycopy(principalBytes, 0, fullSnapshot, 5, principalBytes.length)
    System.arraycopy(stateBytes, 0, fullSnapshot, 5 + principalBytes.length, stateBytes.length)
    fullSnapshot
  }

  private def parseBinaryEnvelopeV2(bytes: Array[Byte]): (Principal, Array[Byte]) = {
    val version = bytes(0) & 0xff
    if (version != 2) throw new RuntimeException(s"Expected version 2, got $version")
    val principalLen =
      ((bytes(1) & 0xff) << 24) | ((bytes(2) & 0xff) << 16) |
        ((bytes(3) & 0xff) << 8) | (bytes(4) & 0xff)
    val principalEnd  = 5 + principalLen
    val principalData = java.util.Arrays.copyOfRange(bytes, 5, principalEnd)
    val p             = PrincipalConverter.fromJson(principalData) match {
      case Right(v)  => v
      case Left(err) => throw new RuntimeException(s"Failed to parse principal: $err")
    }
    val stateBytes = bytes.drop(principalEnd)
    (p, stateBytes)
  }

  def spec = suite("SnapshotEnvelopeSpec")(
    suite("JSON envelope")(
      test("roundtrip with Oidc principal") {
        val stateJson          = """{"counter":42,"label":"hello"}"""
        val envelope           = buildJsonEnvelope(testPrincipal, stateJson)
        val (principal, state) = parseJsonEnvelope(envelope)
        val stateRoundtrip     = Json.parse(stateJson).map(_.print).getOrElse("")
        assertTrue(
          principal == testPrincipal,
          state == stateRoundtrip
        )
      },
      test("roundtrip with Anonymous principal") {
        val stateJson          = """{"value":1}"""
        val envelope           = buildJsonEnvelope(Principal.Anonymous, stateJson)
        val (principal, state) = parseJsonEnvelope(envelope)
        val stateRoundtrip     = Json.parse(stateJson).map(_.print).getOrElse("")
        assertTrue(
          principal == Principal.Anonymous,
          state == stateRoundtrip
        )
      },
      test("roundtrip with Agent principal") {
        val stateJson          = """{"x":"y"}"""
        val envelope           = buildJsonEnvelope(testAgentPrincipal, stateJson)
        val (principal, state) = parseJsonEnvelope(envelope)
        val stateRoundtrip     = Json.parse(stateJson).map(_.print).getOrElse("")
        assertTrue(
          principal == testAgentPrincipal,
          state == stateRoundtrip
        )
      },
      test("envelope contains version field") {
        val envelope = buildJsonEnvelope(Principal.Anonymous, """{}""")
        val parsed   = Json.parse(envelope)
        val version  = parsed.flatMap(_.get("version").one)
        assertTrue(version.isRight)
      }
    ),
    suite("Binary envelope v2")(
      test("roundtrip with Oidc principal") {
        val stateBytes         = Array[Byte](1, 2, 3, 4, 5)
        val envelope           = buildBinaryEnvelopeV2(testPrincipal, stateBytes)
        val (principal, state) = parseBinaryEnvelopeV2(envelope)
        assertTrue(
          principal == testPrincipal,
          state.toSeq == stateBytes.toSeq
        )
      },
      test("roundtrip with Anonymous principal") {
        val stateBytes         = Array[Byte](10, 20, 30)
        val envelope           = buildBinaryEnvelopeV2(Principal.Anonymous, stateBytes)
        val (principal, state) = parseBinaryEnvelopeV2(envelope)
        assertTrue(
          principal == Principal.Anonymous,
          state.toSeq == stateBytes.toSeq
        )
      },
      test("roundtrip with Agent principal") {
        val stateBytes         = Array[Byte](0, 127, -128, 1)
        val envelope           = buildBinaryEnvelopeV2(testAgentPrincipal, stateBytes)
        val (principal, state) = parseBinaryEnvelopeV2(envelope)
        assertTrue(
          principal == testAgentPrincipal,
          state.toSeq == stateBytes.toSeq
        )
      },
      test("envelope starts with version byte 2") {
        val envelope = buildBinaryEnvelopeV2(Principal.Anonymous, Array[Byte](1))
        assertTrue(envelope(0) == 2.toByte)
      },
      test("roundtrip with empty state") {
        val stateBytes         = Array.emptyByteArray
        val envelope           = buildBinaryEnvelopeV2(testPrincipal, stateBytes)
        val (principal, state) = parseBinaryEnvelopeV2(envelope)
        assertTrue(
          principal == testPrincipal,
          state.isEmpty
        )
      },
      test("roundtrip with GolemUser principal") {
        val p = Principal.GolemUser(
          accountId = Uuid(BigInt("111222333444555666"), BigInt("777888999000111222"))
        )
        val stateBytes         = Array[Byte](42)
        val envelope           = buildBinaryEnvelopeV2(p, stateBytes)
        val (principal, state) = parseBinaryEnvelopeV2(envelope)
        assertTrue(
          principal == p,
          state.toSeq == stateBytes.toSeq
        )
      }
    )
  )
}
