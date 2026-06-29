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

package golem.runtime.autowire

import golem.host.SchemaWireInterop
import golem.schema.{IntoSchema, SchemaGraph}
import golem.schema.wire.SchemaWire
import zio.blocks.schema.Schema
import zio.test._

/**
 * Slice 4a — the `golem:core/types@2.0.0` host-payload bridge
 * ([[SchemaPayload]]) is the single hub turning the schema-native typeclasses
 * ([[golem.schema.IntoSchema]] / [[golem.schema.FromSchema]]) into the JS
 * facades the host boundary speaks. These tests pin two invariants:
 *
 *   1. `encode` then `decode` over the full JS bounce is identity for every
 *      representative shape (primitives, records, sealed traits, options,
 *      lists, nesting, recursion).
 *   2. `graph[A]` produces exactly the JS facade of `IntoSchema[A].graph` (so
 *      the schema advertised to the host equals the schema the value encoder
 *      uses).
 */
object SchemaPayloadSpec extends ZIOSpecDefault {

  final case class Point(x: Int, y: Int)
  object Point {
    implicit val schema: Schema[Point] = Schema.derived
  }

  final case class Person(name: String, age: Int, nickname: Option[String], tags: List[String])
  object Person {
    implicit val schema: Schema[Person] = Schema.derived
  }

  sealed trait Shape
  final case class Circle(radius: Double) extends Shape
  final case class Rect(w: Int, h: Int)   extends Shape
  case object Dot                         extends Shape
  object Shape {
    implicit val schema: Schema[Shape] = Schema.derived
  }

  sealed trait Tree
  final case class Leaf(n: Int)                    extends Tree
  final case class Branch(left: Tree, right: Tree) extends Tree
  object Tree {
    implicit val schema: Schema[Tree] = Schema.derived
  }

  private def roundtrip[A](value: A)(implicit into: IntoSchema[A], from: golem.schema.FromSchema[A]) =
    SchemaPayload.decode[A](SchemaPayload.encode(value))

  private def graphEquiv[A](implicit into: IntoSchema[A]): Boolean = {
    val expected = SchemaWireInterop.graphToJs(SchemaWire.schemaGraphToWit(into.graph))
    val actual   = SchemaPayload.graph[A]
    // Both are opaque JS facades; compare via the flat carrier they decode to.
    SchemaWireInterop.graphFromJs(actual) == SchemaWireInterop.graphFromJs(expected)
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaPayloadSpec")(
      suite("encode/decode round-trips through the JS boundary")(
        test("primitive Int") {
          assertTrue(roundtrip(42) == Right(42))
        },
        test("primitive String") {
          assertTrue(roundtrip("hello world") == Right("hello world"))
        },
        test("primitive Boolean") {
          assertTrue(roundtrip(true) == Right(true))
        },
        test("primitive Long (s64 raw-bits boundary)") {
          assertTrue(roundtrip(Long.MaxValue) == Right(Long.MaxValue))
        },
        test("record") {
          assertTrue(roundtrip(Point(3, -7)) == Right(Point(3, -7)))
        },
        test("record with option (some) + list") {
          val p = Person("Ada", 36, Some("Countess"), List("math", "engines"))
          assertTrue(roundtrip(p) == Right(p))
        },
        test("record with option (none) + empty list") {
          val p = Person("Bob", 1, None, Nil)
          assertTrue(roundtrip(p) == Right(p))
        },
        test("sealed trait — record arm") {
          assertTrue(roundtrip[Shape](Circle(2.5)) == Right(Circle(2.5)))
        },
        test("sealed trait — second record arm") {
          assertTrue(roundtrip[Shape](Rect(4, 5)) == Right(Rect(4, 5)))
        },
        test("sealed trait — case object arm") {
          assertTrue(roundtrip[Shape](Dot) == Right(Dot))
        },
        test("recursive type") {
          val t: Tree = Branch(Leaf(1), Branch(Leaf(2), Leaf(3)))
          assertTrue(roundtrip(t) == Right(t))
        },
        test("list of records") {
          val xs = List(Point(0, 0), Point(1, 2), Point(-3, 4))
          assertTrue(roundtrip(xs) == Right(xs))
        }
      ),
      suite("graph[A] equals the JS facade of IntoSchema[A].graph")(
        test("primitive") {
          assertTrue(graphEquiv[Int])
        },
        test("record") {
          assertTrue(graphEquiv[Point])
        },
        test("record with option + list") {
          assertTrue(graphEquiv[Person])
        },
        test("sealed trait") {
          assertTrue(graphEquiv[Shape])
        },
        test("recursive type") {
          assertTrue(graphEquiv[Tree])
        }
      ),
      suite("graphFromModel matches graph[A]")(
        test("graphFromModel(IntoSchema[A].graph) == graph[A]") {
          val a: SchemaGraph = IntoSchema[Person].graph
          val viaModel       = SchemaWireInterop.graphFromJs(SchemaPayload.graphFromModel(a))
          val viaInstance    = SchemaWireInterop.graphFromJs(SchemaPayload.graph[Person])
          assertTrue(viaModel == viaInstance)
        }
      )
    )
}
