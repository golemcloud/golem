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

package golem.runtime.autowire

import zio.test._

import scala.scalajs.js
import zio.blocks.schema.Schema

private[autowire] object JsPlainSchemaCodecSpecTypes {
  final case class Nested(x: Double, tags: List[String])
  object Nested {
    implicit val schema: Schema[Nested] = Schema.derived
  }

  final case class Payload(
    name: String,
    count: Int,
    note: Option[String],
    flags: List[String],
    nested: Nested
  )
  object Payload {
    implicit val schema: Schema[Payload] = Schema.derived
  }
}

object JsPlainSchemaCodecSpec extends ZIOSpecDefault {
  import JsPlainSchemaCodecSpecTypes._

  def spec = suite("JsPlainSchemaCodecSpec")(
    test("roundtrip: Scala value -> JS plain -> Scala value") {
      val v = Payload("abc", 7, Some("n"), List("x", "y", "z"), Nested(1.5, List("a", "b")))

      val jsAny = JsPlainSchemaCodec.encode(v)
      val back  = JsPlainSchemaCodec.decode[Payload](jsAny)

      assertTrue(back == Right(v))
    },
    test("decode from manual JS object (null option)") {
      val jsObj =
        js.Dynamic.literal(
          "name"   -> "abc",
          "count"  -> 7,
          "note"   -> null,
          "flags"  -> js.Array("x", "y"),
          "nested" -> js.Dynamic.literal("x" -> 1.5, "tags" -> js.Array("a", "b"))
        )

      val got = JsPlainSchemaCodec.decode[Payload](jsObj.asInstanceOf[js.Any])

      assertTrue(
        got == Right(
          Payload(
            name = "abc",
            count = 7,
            note = None,
            flags = List("x", "y"),
            nested = Nested(1.5, List("a", "b"))
          )
        )
      )
    }
  )
}
