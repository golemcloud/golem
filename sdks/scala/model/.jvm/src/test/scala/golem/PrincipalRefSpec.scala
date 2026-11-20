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

package golem

import zio.blocks.schema.json.Json
import zio.test._

object PrincipalRefSpec extends ZIOSpecDefault {

  /**
   * The shared text-form vectors
   * (`test-data/caller-identity/text-form-v1.json`).
   */
  private lazy val vectors: Json = {
    val input = getClass.getResourceAsStream("/text-form-v1.json")
    require(input != null, "the shared PrincipalRef vectors are missing")
    val bytes =
      try input.readAllBytes()
      finally input.close()
    Json.parse(bytes).fold(error => throw new RuntimeException(error.toString), identity)
  }

  private def entries(name: String): List[Json] =
    vectors.get(name).one.toOption.collect { case array: Json.Array => array.value.toList }.getOrElse(Nil)

  private def text(entry: Json, field: String): String =
    entry.get(field).one.toOption.collect { case s: Json.String => s.value }.getOrElse(sys.error(s"missing $field"))

  def spec = suite("PrincipalRefSpec")(
    test("prints and parses exactly the shared vectors") {
      val roundTrip = entries("roundTrip")
      assertTrue(roundTrip.nonEmpty) &&
      assertTrue(roundTrip.forall { v =>
        val ref = PrincipalRef(text(v, "issuer"), text(v, "subject"))
        ref.toString == text(v, "text") && PrincipalRef.parse(text(v, "text")) == ref
      })
    },
    test("refuses every non-canonical spelling") {
      val reject = entries("reject")
      assertTrue(reject.nonEmpty) &&
      assertTrue(reject.forall(v => PrincipalRef.parseEither(text(v, "text")).isLeft))
    },
    test("compares both strings byte for byte") {
      val base = PrincipalRef("https://login.example/", "Subject-1")
      assertTrue(
        base == PrincipalRef("https://login.example/", "Subject-1"),
        base != PrincipalRef("https://login.example", "Subject-1"),
        base != PrincipalRef("https://login.example/", "subject-1")
      )
    },
    test("an anonymous principal has no reference and is the same as nothing") {
      val user = Principal.User(PrincipalRef("i", "s"), "{}", java.time.Instant.EPOCH)
      assertTrue(
        Principal.Anonymous.ref.isEmpty,
        !Principal.Anonymous.isSameAs(Principal.Anonymous),
        !user.isSameAs(Principal.Anonymous),
        user.isSameAs(PrincipalRef("i", "s")),
        user.toString == """user ("i","s")""",
        Principal.Anonymous.toString == "anonymous"
      )
    }
  )
}
