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

package golem.runtime.rpc

import golem.data.GolemSchema
import golem.host.js._
import zio.test._
import zio.blocks.schema.Schema

import java.util.UUID
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

  private def rpcRoundTripTests[A: Schema](label: String, iterations: Int)(gen: => A): Spec[Any, Nothing] = {
    implicit val gs: GolemSchema[A] = GolemSchema.fromBlocksSchema[A]
    test(s"rpc roundtrip fuzz: $label ($iterations cases)") {
      var i = 0
      while (i < iterations) {
        val in        = gen
        val dataValue = RpcValueCodec.encodeArgs(in).fold(err => throw new RuntimeException(err), identity)
        val witValue  =
          dataValue.asInstanceOf[JsDataValueTuple].value(0).asInstanceOf[JsElementValueComponentModel].value
        val out = RpcValueCodec.decodeValue[A](witValue).fold(err => throw new RuntimeException(err), identity)
        Predef.assert(out == in)
        i += 1
      }
      assertCompletes
    }
  }

  def spec = suite("ValueMappingFuzzSpec")(
    rpcRoundTripTests[Int]("int", 200)(rng.nextInt()),
    rpcRoundTripTests[String]("string", 200)(genString(64)),
    rpcRoundTripTests[Option[Int]]("option", 200)(if (rng.nextBoolean()) Some(rng.nextInt(1000)) else None),
    rpcRoundTripTests[List[String]]("list", 150)(List.fill(rng.nextInt(6))(genString(12))),
    rpcRoundTripTests[Map[String, Int]]("map", 150)(
      (0 until rng.nextInt(6)).map(_ => genString(6) -> rng.nextInt(100)).toMap
    ),
    rpcRoundTripTests[TinyProduct]("product", 150)(genTinyProduct()),
    rpcRoundTripTests[TinySum]("sum", 150)(genTinySum()),
    rpcRoundTripTests[UUID]("uuid", 100)(new UUID(rng.nextLong(), rng.nextLong()))
  )
}
