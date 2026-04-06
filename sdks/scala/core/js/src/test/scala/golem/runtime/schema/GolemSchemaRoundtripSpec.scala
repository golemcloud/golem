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

package golem.runtime.schema

import golem.data.GolemSchema
import zio.test._
import zio.blocks.schema.Schema

private[schema] object GolemSchemaRoundtripTypes {
  final case class Person(name: String, age: Int, tags: List[String])
  object Person { implicit val schema: Schema[Person] = Schema.derived }

  sealed trait Color
  object Color {
    case object Red  extends Color
    case object Blue extends Color
    implicit val schema: Schema[Color] = Schema.derived
  }
}

object GolemSchemaRoundtripSpec extends ZIOSpecDefault {
  import GolemSchemaRoundtripTypes._

  private def roundTripTest[A](label: String)(value: A)(implicit schema: Schema[A]): Spec[Any, Nothing] = {
    implicit val gs: GolemSchema[A] = GolemSchema.fromBlocksSchema[A]
    test(s"roundtrip: $label") {
      val encoded = gs.encode(value).fold(err => throw new RuntimeException(err), identity)
      val decoded = gs.decode(encoded).fold(err => throw new RuntimeException(err), identity)
      assertTrue(decoded == value)
    }
  }

  private implicit val tuple2Schema: Schema[(String, Int)]          = Schema.derived
  private implicit val tuple3Schema: Schema[(String, Int, Boolean)] = Schema.derived

  def spec = suite("GolemSchemaRoundtripSpec")(
    roundTripTest("unit")(()),
    roundTripTest("string")("hello"),
    roundTripTest("int")(123),
    roundTripTest("option some")(Option("x")),
    roundTripTest("option none")(Option.empty[String]),
    roundTripTest("list")(List(1, 2, 3)),
    roundTripTest("map")(Map("a" -> 1, "b" -> 2)),
    roundTripTest("product")(Person("alice", 42, List("x", "y")))(Person.schema),
    roundTripTest[Color]("enum")(Color.Blue)(Color.schema),
    roundTripTest("tuple2")(("a", 1)),
    roundTripTest("tuple3")(("a", 1, true))
  )
}
