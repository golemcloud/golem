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

package golem.runtime.rpc

import golem.Uuid
import golem.runtime.autowire.SchemaPayload
import golem.schema.{FromSchema, IntoSchema}
import zio.test._
import zio.blocks.schema.Schema

import scala.util.Random

private[rpc] object ValueMappingFuzzSpecTypes {
  final case class TinyProduct(a: Int, b: String, c: Option[Int], d: List[String])
  object TinyProduct { implicit val schema: Schema[TinyProduct] = Schema.derived }

  sealed trait TinySum
  object TinySum {
    case object A                      extends TinySum
    final case class B(i: Int)         extends TinySum
    final case class C(p: TinyProduct) extends TinySum
    implicit val schema: Schema[TinySum] = Schema.derived
  }
}

/**
 * Randomised round-trip fuzz over the `golem:core/types@2.0.0` value boundary:
 * `IntoSchema[A].toValue` -> `SchemaWire` -> JS `Js*` facade ->
 * `FromSchema[A].fromValue` (the [[SchemaPayload]] hub the host uses).
 */
object ValueMappingFuzzSpec extends ZIOSpecDefault {
  import ValueMappingFuzzSpecTypes._

  private val rng = new Random(0xc0ffee)

  private def genString(max: Int): String = {
    val n  = rng.nextInt(max + 1)
    val sb = new StringBuilder(n)
    var i  = 0
    while (i < n) {
      val ch = ('a'.toInt + rng.nextInt(26)).toChar
      sb.append(ch)
      i += 1
    }
    sb.result()
  }

  private def genTinyProduct(): TinyProduct =
    TinyProduct(
      a = rng.nextInt(1000) - 500,
      b = genString(16),
      c = if (rng.nextBoolean()) Some(rng.nextInt(100)) else None,
      d = List.fill(rng.nextInt(5))(genString(8))
    )

  private def genTinySum(): TinySum =
    rng.nextInt(3) match {
      case 0 => TinySum.A
      case 1 => TinySum.B(rng.nextInt(1000))
      case _ => TinySum.C(genTinyProduct())
    }

  private def roundTripTests[A: Schema](label: String, iterations: Int)(gen: => A): Spec[Any, Nothing] = {
    implicit val into: IntoSchema[A] = IntoSchema.derived
    implicit val from: FromSchema[A] = FromSchema.derived
    test(s"schema value roundtrip fuzz: $label ($iterations cases)") {
      var i = 0
      while (i < iterations) {
        val in   = gen
        val tree = SchemaPayload.encode[A](in)
        val out  = SchemaPayload.decode[A](tree).fold(err => throw new RuntimeException(err.toString), identity)
        Predef.assert(out == in, s"roundtrip mismatch: in=$in out=$out")
        i += 1
      }
      assertCompletes
    }
  }

  def spec = suite("ValueMappingFuzzSpec")(
    roundTripTests[Int]("int", 200)(rng.nextInt()),
    roundTripTests[String]("string", 200)(genString(64)),
    roundTripTests[Option[Int]]("option", 200)(if (rng.nextBoolean()) Some(rng.nextInt(1000)) else None),
    roundTripTests[List[String]]("list", 150)(List.fill(rng.nextInt(6))(genString(12))),
    roundTripTests[Map[String, Int]]("map", 150)(
      (0 until rng.nextInt(6)).map(_ => genString(6) -> rng.nextInt(100)).toMap
    ),
    roundTripTests[TinyProduct]("product", 150)(genTinyProduct()),
    roundTripTests[TinySum]("sum", 150)(genTinySum()),
    roundTripTests[Uuid]("uuid", 100)(
      Uuid(
        BigInt(java.lang.Long.toUnsignedString(rng.nextLong())),
        BigInt(java.lang.Long.toUnsignedString(rng.nextLong()))
      )
    )
  )
}
