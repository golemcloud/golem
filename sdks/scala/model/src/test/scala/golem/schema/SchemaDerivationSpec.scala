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

package golem.schema

import golem.{EnvironmentId, Uuid}
import golem.config.Secret
import golem.{UByte, UInt, ULong, UShort}
import zio.blocks.schema.Schema
import zio.test.Assertion._
import zio.test._

import scala.util.Try

object SchemaDerivationSpec extends ZIOSpecDefault {
  import SchemaTypeBody._

  // ---- test types (defined at object scope for Scala 2.13 + 3 derivation) ----

  final case class Prims(
    b: Boolean,
    i8: Byte,
    i16: Short,
    i32: Int,
    i64: Long,
    f: Float,
    d: Double,
    c: Char,
    s: String,
    bd: BigDecimal,
    bi: BigInt
  )
  object Prims {
    implicit val schema: Schema[Prims] = Schema.derived
  }

  final case class Point(x: Int, y: Int)
  object Point {
    implicit val schema: Schema[Point] = Schema.derived
  }

  sealed trait Shape
  final case class Circle(radius: Double) extends Shape
  final case class Rect(w: Int, h: Int)   extends Shape
  case object Unit_                       extends Shape
  object Shape {
    implicit val schema: Schema[Shape] = Schema.derived
  }

  sealed trait Color
  case object Red   extends Color
  case object Green extends Color
  case object Blue  extends Color
  object Color {
    implicit val schema: Schema[Color] = Schema.derived
  }

  final case class Box[T](value: T)
  // zio-blocks 0.0.32 bakes the abstract type param into a generic
  // `Schema.derived` in a `def [T: Schema]`, so generic instantiations must be
  // derived concretely to carry concrete type args + a working value encoder.
  implicit val boxIntSchema: Schema[Box[Int]]    = Schema.derived
  implicit val boxStrSchema: Schema[Box[String]] = Schema.derived

  sealed trait Tree
  final case class Leaf(n: Int)                    extends Tree
  final case class Branch(left: Tree, right: Tree) extends Tree
  object Tree {
    implicit val schema: Schema[Tree] = Schema.derived
  }

  final case class Ping(next: Option[Pong])
  final case class Pong(next: Option[Ping])
  object Ping {
    implicit val schema: Schema[Ping] = Schema.derived
  }
  object Pong {
    implicit val schema: Schema[Pong] = Schema.derived
  }

  // zio-blocks ships built-in `Schema[Option]`/`List`/`Set`/`Map` but cannot
  // derive stdlib `Either`/tuple schemas on Scala 2.13, so those derivation
  // cases are covered in the Scala-3-only `SchemaDerivationScala3Spec`.

  // ---- helpers ----

  private def rootBody[A](implicit s: IntoSchema[A]): SchemaTypeBody = s.graph.root.body

  private def refId(t: SchemaType): String =
    t.body match {
      case RefType(id) => id
      case other       => throw new AssertionError(s"expected a ref, got $other")
    }

  private def defBody(g: SchemaGraph, id: String): SchemaTypeBody =
    g.defs(id).body.body

  private def roundTrip[A](value: A)(implicit into: IntoSchema[A], from: FromSchema[A]): Either[FromSchemaError, A] =
    from.fromValue(into.toValue(value))

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaDerivationSpec")(
      // -------------------------------------------------------------------
      // primitives + scalars
      // -------------------------------------------------------------------
      test("derives all primitives (BigDecimal/BigInt as string) and round-trips") {
        val g      = IntoSchema[Prims].graph
        val body   = defBody(g, refId(g.root))
        val fields = body match {
          case RecordType(fs) => fs.map(f => f.name -> f.body.body).toMap
          case other          => throw new AssertionError(other.toString)
        }
        val v       = Prims(true, 1, 2, 3, 4L, 1.5f, 2.5d, 'q', "hi", BigDecimal("3.14"), BigInt("123456789012345"))
        val decoded = roundTrip(v)

        assertTrue(
          fields("b") == BoolType,
          fields("i8") == S8Type(),
          fields("i16") == S16Type(),
          fields("i32") == S32Type(),
          fields("i64") == S64Type(),
          fields("f") == F32Type(),
          fields("d") == F64Type(),
          fields("c") == CharType,
          fields("s") == StringType,
          fields("bd") == StringType,
          fields("bi") == StringType
        ) && assert(decoded)(isRight(equalTo(v)))
      },
      test("Unit derives as empty tuple and round-trips") {
        val into = IntoSchema[Unit]
        assertTrue(into.graph.root.body == TupleType(Nil), into.toValue(()) == SchemaValue.TupleValue(Nil)) &&
        assert(roundTrip(()))(isRight(equalTo(())))
      },
      // -------------------------------------------------------------------
      // records
      // -------------------------------------------------------------------
      test("case class derives a nominal record def referenced by the root") {
        val g  = IntoSchema[Point].graph
        val id = refId(g.root)
        assertTrue(
          g.defs.contains(id),
          g.defs(id).name.contains("Point"),
          defBody(g, id) == RecordType(List(NamedFieldType("x", t.s32), NamedFieldType("y", t.s32)))
        ) && assert(roundTrip(Point(3, 7)))(isRight(equalTo(Point(3, 7))))
      },
      // -------------------------------------------------------------------
      // variants + enums
      // -------------------------------------------------------------------
      test("mixed sealed trait derives a variant; all-empty derives an enum") {
        val gShape    = IntoSchema[Shape].graph
        val gColor    = IntoSchema[Color].graph
        val shapeBody = defBody(gShape, refId(gShape.root))
        val colorBody = defBody(gColor, refId(gColor.root))

        val shapeIsVariant = shapeBody match {
          case VariantType(cases) => cases.map(_.name).toSet == Set("Circle", "Rect", "Unit_")
          case _                  => false
        }
        val colorIsEnum = colorBody match {
          case EnumType(cases) => cases == List("Red", "Green", "Blue")
          case _               => false
        }
        assertTrue(shapeIsVariant, colorIsEnum)
      },
      test("variant values round-trip (payload, record payload, no payload)") {
        val c: Shape = Circle(2.5)
        val r: Shape = Rect(4, 5)
        val u: Shape = Unit_
        assert(roundTrip(c))(isRight(equalTo(c))) &&
        assert(roundTrip(r))(isRight(equalTo(r))) &&
        assert(roundTrip(u))(isRight(equalTo(u)))
      },
      test("no-payload case in a mixed variant encodes as VariantValue(_, None), not EnumValue") {
        val gShape = IntoSchema[Shape].graph
        val cases  = defBody(gShape, refId(gShape.root)) match {
          case VariantType(cs) => cs.map(_.name)
          case other           => throw new AssertionError(other.toString)
        }
        val idx     = cases.indexOf("Unit_")
        val encoded = IntoSchema[Shape].toValue(Unit_)
        assertTrue(idx >= 0, encoded == SchemaValue.VariantValue(idx, None))
      },
      test("enum values round-trip") {
        assert(roundTrip[Color](Red))(isRight(equalTo(Red: Color))) &&
        assert(roundTrip[Color](Blue))(isRight(equalTo(Blue: Color)))
      },
      test("no-payload case in a pure enum encodes as EnumValue") {
        val gColor = IntoSchema[Color].graph
        val cases  = defBody(gColor, refId(gColor.root)) match {
          case EnumType(cs) => cs
          case other        => throw new AssertionError(other.toString)
        }
        val idx     = cases.indexOf("Green")
        val encoded = IntoSchema[Color].toValue(Green)
        assertTrue(idx >= 0, encoded == SchemaValue.EnumValue(idx))
      },
      // -------------------------------------------------------------------
      // option / either / collections / tuples
      // -------------------------------------------------------------------
      test("Option derives option and round-trips Some/None") {
        assertTrue(rootBody[Option[Int]] == OptionType(t.s32)) &&
        assert(roundTrip[Option[Int]](Some(9)))(isRight(equalTo(Some(9): Option[Int]))) &&
        assert(roundTrip[Option[Int]](None))(isRight(equalTo(None: Option[Int])))
      },
      test("List/Set derive list; Map derives map") {
        assertTrue(
          rootBody[List[Int]] == ListType(t.s32),
          rootBody[Set[Int]] == ListType(t.s32),
          rootBody[Map[String, Int]] == MapType(t.string, t.s32)
        )
      },
      test("collection values round-trip") {
        assert(roundTrip(List(1, 2, 3)))(isRight(equalTo(List(1, 2, 3)))) &&
        assert(roundTrip(Set(1, 2, 3)))(isRight(equalTo(Set(1, 2, 3)))) &&
        assert(roundTrip(Map("a" -> 1, "b" -> 2)))(isRight(equalTo(Map("a" -> 1, "b" -> 2))))
      },
      // -------------------------------------------------------------------
      // generics
      // -------------------------------------------------------------------
      test("generic instantiations get distinct, generic-aware type ids") {
        val idInt = refId(IntoSchema[Box[Int]].graph.root)
        val idStr = refId(IntoSchema[Box[String]].graph.root)
        assertTrue(idInt != idStr, idInt.contains("Int"), idStr.contains("String")) &&
        assert(roundTrip(Box(5)))(isRight(equalTo(Box(5)))) &&
        assert(roundTrip(Box("z")))(isRight(equalTo(Box("z"))))
      },
      // -------------------------------------------------------------------
      // recursion
      // -------------------------------------------------------------------
      test("self-recursive type closes via refs and round-trips") {
        val g          = IntoSchema[Tree].graph
        val id         = refId(g.root)
        val tree: Tree = Branch(Leaf(1), Branch(Leaf(2), Leaf(3)))
        assertTrue(g.defs.contains(id)) &&
        assert(roundTrip(tree))(isRight(equalTo(tree)))
      },
      test("mutually-recursive types round-trip") {
        val p: Ping = Ping(Some(Pong(Some(Ping(None)))))
        assert(roundTrip(p))(isRight(equalTo(p)))
      },
      // -------------------------------------------------------------------
      // built-ins: Uuid + unsigned wrappers
      // -------------------------------------------------------------------
      test("Uuid derives the canonical cross-SDK record and round-trips") {
        val g  = IntoSchema[Uuid].graph
        val id = refId(g.root)
        assertTrue(
          id == "uuid.Uuid",
          g.defs(id).name.contains("uuid"),
          defBody(g, id) == RecordType(List(NamedFieldType("high-bits", t.u64), NamedFieldType("low-bits", t.u64)))
        ) && assert(
          Uuid.fromStandardString("12345678-1234-5678-1234-567812345678").flatMap(roundTrip(_).left.map(_.getMessage))
        )(isRight(equalTo(Uuid.fromStandardString("12345678-1234-5678-1234-567812345678").toOption.get)))
      },
      test("EnvironmentId derives a record over the canonical Uuid built-in and round-trips") {
        val g       = IntoSchema[EnvironmentId].graph
        val id      = refId(g.root)
        val uuidRef = defBody(g, id) match {
          case RecordType(List(NamedFieldType("uuid", body, _))) => refId(body)
          case other                                             => throw new AssertionError(other.toString)
        }
        val envId = EnvironmentId(Uuid.fromStandardString("12345678-1234-5678-1234-567812345678").toOption.get)
        assertTrue(g.defs.contains(id), uuidRef == "uuid.Uuid") &&
        assert(roundTrip(envId))(isRight(equalTo(envId)))
      },
      test("Uuid value encodes high/low as raw u64 bits") {
        val maxHi   = Uuid(BigInt("18446744073709551615"), BigInt(0)) // 2^64 - 1, 0
        val encoded = IntoSchema[Uuid].toValue(maxHi)
        assertTrue(
          encoded == SchemaValue.RecordValue(List(SchemaValue.U64Value(-1L), SchemaValue.U64Value(0L)))
        ) && assert(roundTrip(maxHi))(isRight(equalTo(maxHi)))
      },
      test("unsigned wrappers derive u8/u16/u32/u64 and round-trip (incl. boundary)") {
        assertTrue(
          rootBody[UByte] == U8Type(),
          rootBody[UShort] == U16Type(),
          rootBody[UInt] == U32Type(),
          rootBody[ULong] == U64Type()
        ) &&
        assert(roundTrip(UByte(255)))(isRight(equalTo(UByte(255)))) &&
        assert(roundTrip(UShort(65535)))(isRight(equalTo(UShort(65535)))) &&
        assert(roundTrip(UInt(4294967295L)))(isRight(equalTo(UInt(4294967295L)))) &&
        assert(roundTrip(ULong(BigInt("18446744073709551615"))))(
          isRight(equalTo(ULong(BigInt("18446744073709551615"))))
        )
      },
      test("Uuid encode rejects out-of-range high/low bits with SchemaEncodeError") {
        val negHi  = Try(IntoSchema[Uuid].toValue(Uuid(BigInt(-1), BigInt(0))))
        val overLo = Try(IntoSchema[Uuid].toValue(Uuid(BigInt(0), BigInt(1) << 64)))
        val overHi = Try(IntoSchema[Uuid].toValue(Uuid((BigInt(1) << 64) + 123, BigInt(0))))
        assertTrue(
          negHi.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          overLo.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          overHi.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError])
        )
      },
      test("char decode rejects out-of-Char-range code points with FromSchemaError") {
        assert(FromSchema[Char].fromValue(SchemaValue.CharValue(0x1f600)))(isLeft) &&
        assert(FromSchema[Char].fromValue(SchemaValue.CharValue(-1)))(isLeft) &&
        assert(roundTrip('q'))(isRight(equalTo('q')))
      },
      test("malformed BigInt/BigDecimal strings decode to FromSchemaError (NonFatal boundary)") {
        assert(FromSchema[BigInt].fromValue(SchemaValue.StringValue("not-a-number")))(isLeft) &&
        assert(FromSchema[BigDecimal].fromValue(SchemaValue.StringValue("12.x")))(isLeft) &&
        assert(roundTrip(BigInt("123456789012345")))(isRight(equalTo(BigInt("123456789012345"))))
      },
      test("unsigned decode rejects out-of-range values with FromSchemaError") {
        assert(FromSchema[UByte].fromValue(SchemaValue.U8Value(256)))(isLeft) &&
        assert(FromSchema[UByte].fromValue(SchemaValue.U8Value(-1)))(isLeft) &&
        assert(FromSchema[UShort].fromValue(SchemaValue.U16Value(65536)))(isLeft) &&
        assert(FromSchema[UShort].fromValue(SchemaValue.U16Value(-1)))(isLeft) &&
        assert(FromSchema[UInt].fromValue(SchemaValue.U32Value(4294967296L)))(isLeft) &&
        assert(FromSchema[UInt].fromValue(SchemaValue.U32Value(-1L)))(isLeft)
      },
      test("unsigned encode rejects out-of-range wrapper values with SchemaEncodeError") {
        val u8  = Try(IntoSchema[UByte].toValue(UByte(300)))
        val u8n = Try(IntoSchema[UByte].toValue(UByte(-1)))
        val u16 = Try(IntoSchema[UShort].toValue(UShort(70000)))
        val u32 = Try(IntoSchema[UInt].toValue(UInt(5000000000L)))
        val u64 = Try(IntoSchema[ULong].toValue(ULong(BigInt("18446744073709551616"))))
        assertTrue(
          u8.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          u8n.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          u16.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          u32.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          u64.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError])
        )
      },
      // -------------------------------------------------------------------
      // fail-loud: Secret
      // -------------------------------------------------------------------
      test("Secret fails loud rather than silently unwrapping") {
        val graphAttempt = Try(IntoSchema[Secret[String]].graph)
        val valueAttempt = Try(IntoSchema[Secret[String]].toValue(new Secret[String](Nil, () => "s")))
        assertTrue(
          graphAttempt.isFailure,
          graphAttempt.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError]),
          valueAttempt.isFailure
        )
      },
      // -------------------------------------------------------------------
      // error paths
      // -------------------------------------------------------------------
      test("decoding a mismatched value tree fails with FromSchemaError") {
        val wrongOption = FromSchema[Option[Int]].fromValue(SchemaValue.StringValue("nope"))
        val wrongRecord = FromSchema[Point].fromValue(SchemaValue.RecordValue(List(SchemaValue.S32Value(1))))
        val wrongIndex  = FromSchema[Color].fromValue(SchemaValue.EnumValue(99))
        assert(wrongOption)(isLeft) && assert(wrongRecord)(isLeft) && assert(wrongIndex)(isLeft)
      }
    )
}
