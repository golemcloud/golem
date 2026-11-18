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

package golem.runtime

import golem.runtime.guest.Guest
import zio.blocks.schema.json.Json
import zio.test._

object SnapshotEnvelopeSpec extends ZIOSpecDefault {

  // The principals that 1.6 wrote into a snapshot, in their JSON form.
  private val legacyOidc =
    """{"tag":"oidc","val":{"sub":"user-42","issuer":"https://auth.example.com","email":"user@example.com","name":null,"emailVerified":null,"givenName":null,"familyName":null,"picture":null,"preferredUsername":null,"claims":"{\"role\":\"admin\"}"}}"""
  private val legacyAgent =
    """{"tag":"agent","val":{"componentId":"550e8400-e29b-41d4-a716-446655440000","agentId":"agent-1"}}"""
  private val legacyUser      = """{"tag":"golem-user","val":{"accountId":"550e8400-e29b-41d4-a716-446655440000"}}"""
  private val legacyAnonymous = """{"tag":"anonymous"}"""

  private def state(bytes: Array[Byte]): String = new String(bytes, "UTF-8")

  private def decodeJson(text: String): Either[String, String] =
    Guest
      .decodeSnapshotPayload(text.getBytes("UTF-8"), "application/json")
      .map(data => state(data.asInstanceOf[SnapshotPayload].bytes))

  private def printed(json: String): String = Json.parse(json).map(_.print).getOrElse("")

  /**
   * A 1.6 binary envelope: `0x02`, the principal length, the principal JSON,
   * then the state.
   */
  private def legacyBinary(principalJson: String, stateBytes: Array[Byte]): Array[Byte] = {
    val principalBytes = principalJson.getBytes("UTF-8")
    val out            = new Array[Byte](1 + 4 + principalBytes.length + stateBytes.length)
    out(0) = 2.toByte
    out(1) = ((principalBytes.length >>> 24) & 0xff).toByte
    out(2) = ((principalBytes.length >>> 16) & 0xff).toByte
    out(3) = ((principalBytes.length >>> 8) & 0xff).toByte
    out(4) = (principalBytes.length & 0xff).toByte
    System.arraycopy(principalBytes, 0, out, 5, principalBytes.length)
    System.arraycopy(stateBytes, 0, out, 5 + principalBytes.length, stateBytes.length)
    out
  }

  private def decodeBinary(bytes: Array[Byte]): Either[String, Seq[Byte]] =
    Guest
      .decodeSnapshotPayload(bytes, "application/octet-stream")
      .map(_.asInstanceOf[SnapshotPayload].bytes.toSeq)

  def spec = suite("SnapshotEnvelopeSpec")(
    suite("JSON envelope")(
      test("reads the 2.0 envelope, which holds no principal") {
        val stateJson = """{"counter":42,"label":"hello"}"""
        assertTrue(decodeJson(s"""{"version":2,"state":$stateJson}""") == Right(printed(stateJson)))
      },
      test("refuses a 1.6 envelope as an unknown version") {
        val forms = List(legacyOidc, legacyAgent, legacyUser, legacyAnonymous)
        assertTrue(forms.forall { principal =>
          decodeJson(s"""{"version":1,"principal":$principal,"state":{"x":"y"}}""").left
            .exists(_.contains("Unsupported JSON snapshot version: 1"))
        })
      },
      test("loader rejects missing version and state") {
        val noVersion = decodeJson("""{"state":{}}""")
        val noState   = decodeJson("""{"version":2}""")
        assertTrue(noVersion.left.exists(_.contains("version")), noState.left.exists(_.contains("state")))
      },
      test("loader rejects an unsupported version") {
        assertTrue(decodeJson("""{"version":3,"state":{}}""").left.exists(_.contains("version")))
      }
    ),
    suite("Binary envelope")(
      test("reads the 2.0 envelope: the version byte 3, then the state") {
        assertTrue(decodeBinary(Array[Byte](3, 1, 2, 3)) == Right(Seq[Byte](1, 2, 3)))
      },
      test("reads an empty 2.0 state") {
        assertTrue(decodeBinary(Array[Byte](3)) == Right(Seq.empty[Byte]))
      },
      test("refuses a 1.6 binary v2 envelope as an unknown version") {
        val forms = List(legacyOidc, legacyAgent, legacyUser, legacyAnonymous)
        assertTrue(
          forms.forall(principal =>
            decodeBinary(legacyBinary(principal, Array[Byte](9, 8))).left
              .exists(_.contains("Unsupported snapshot version: 2"))
          )
        )
      },
      test("refuses the 1.6 binary v1 envelope as an unknown version") {
        assertTrue(decodeBinary(Array[Byte](1, 10, 20)).left.exists(_.contains("Unsupported snapshot version: 1")))
      },
      test("rejects empty, truncated, oversized, invalid-principal, and unknown envelopes") {
        val empty            = Guest.decodeSnapshotPayload(Array.emptyByteArray, "application/octet-stream")
        val truncated        = Guest.decodeSnapshotPayload(Array[Byte](2, 0), "application/octet-stream")
        val oversized        = Guest.decodeSnapshotPayload(Array[Byte](2, 0, 0, 0, 2, 1), "application/octet-stream")
        val invalidPrincipal = Guest.decodeSnapshotPayload(Array[Byte](2, 0, 0, 0, 1, 0), "application/octet-stream")
        val unknown          = Guest.decodeSnapshotPayload(Array[Byte](4), "application/octet-stream")
        assertTrue(empty.isLeft, truncated.isLeft, oversized.isLeft, invalidPrincipal.isLeft, unknown.isLeft)
      }
    )
  )
}
