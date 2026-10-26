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

import golem.schema.wire.*
import golem.schema.Implicits.golemUriSchema
import zio.ZIO
import zio.blocks.schema.Schema
import zio.test.*

import java.net.URI
import java.util.UUID
import scala.concurrent.Future
import scala.util.Try

object ConcreteCodecSpec extends ZIOSpecDefault {
  final case class Item(label: String, counts: List[Either[String, Int]], next: Option[Item])
  object Item { implicit lazy val schema: Schema[Item] = Schema.derived }
  enum Choice {
    case Empty
    case Scalar(value: Int)
    case Fields(name: String, count: Int)
  }
  object Choice { implicit val schema: Schema[Choice] = Schema.derived }
  final case class UuidHolder(id: UUID, parent: Option[UUID], children: List[UUID])
  object UuidHolder { implicit val schema: Schema[UuidHolder] = Schema.derived }
  sealed trait Animal
  sealed trait Pet                    extends Animal
  final case class Dog(name: String)  extends Pet
  case object Cat                     extends Pet
  final case class Wild(kind: String) extends Animal
  object Animal { implicit val schema: Schema[Animal] = Schema.derived }
  final case class Timed(at: java.time.Instant, took: java.time.Duration, history: List[java.time.Instant])
  object Timed { implicit val schema: Schema[Timed] = Schema.derived }
  type Wide = (
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    String
  )
  implicit val wideSchema: Schema[Wide] = Schema.derived

  final case class Located(path: GolemPath, url: URI)
  object Located { implicit val schema: Schema[Located] = Schema.derived }

  private def decodeFailure[A](result: Try[A]): Boolean =
    result.failed.toOption.exists(_.isInstanceOf[SchemaDecodeError])
  private def encodeFailure[A](result: Try[A]): Boolean =
    result.failed.toOption.exists(_.isInstanceOf[SchemaEncodeError])

  // Scala.js already rejects a lone surrogate when the URI is constructed; the JVM accepts it and the encoder rejects it.
  private def uriEncodeFailure[A](text: String)(encode: URI => A): Boolean =
    Try(new URI(text)).fold(_.isInstanceOf[java.net.URISyntaxException], uri => encodeFailure(Try(encode(uri))))

  override def spec = suite("ConcreteCodecSpec")(
    test("recursive concrete values and named descriptors match the wire contract") {
      val codec   = ConcreteCodec.derived[Item]
      val value   = Item("first", List(Right(17), Left("bad")), Some(Item("last", Nil, None)))
      val encoded = codec.encode(value)
      assertTrue(
        codec.decode(encoded.value) == value,
        SchemaWire.schemaGraphFromWit(encoded.graph) == IntoSchema[Item].graph,
        SchemaWire.schemaValueFromWit(encoded.value) == IntoSchema[Item].toValue(value)
      )
    },
    test("variants preserve unit, unwrapped value and record cases") {
      val codec  = ConcreteCodec.derived[Choice]
      val values = List(Choice.Empty, Choice.Scalar(23), Choice.Fields("blue", -8))
      assertTrue(
        values.forall { value =>
          val encoded = codec.encode(value)
          codec.decode(encoded.value) == value &&
          SchemaWire.schemaValueFromWit(encoded.value) == IntoSchema[Choice].toValue(value)
        },
        SchemaWire.schemaGraphFromWit(codec.graph) == IntoSchema[Choice].graph
      )
    },
    test("rich values, unsigned boundaries and collection carriers remain concrete") {
      val paths     = ConcreteCodec.derived[Map[String, Seq[GolemPath]]]
      val value     = Map("first" -> Seq(new GolemPath("/a"), new GolemPath("relative")), "empty" -> Seq.empty)
      val array     = ConcreteCodec.derived[Array[Option[Int]]]
      val unsigned  = ConcreteCodec.derived[(golem.UInt, golem.ULong)]
      val large     = (new golem.UInt(0xffffffffL), new golem.ULong((BigInt(1) << 64) - 1))
      val malformed = WitSchemaValueTree(Vector(WitSchemaValueNode.U32Value(-1)), 0)
      val datetime  = ConcreteCodec.derived[java.time.Instant]
      assertTrue(
        paths.decode(paths.encodeValue(value)) == value,
        array.decode(array.encodeValue(Array(Some(3), None, Some(-7)))).toList == List(Some(3), None, Some(-7)),
        unsigned.decode(unsigned.encodeValue(large)) == large,
        Try(ConcreteCodec.uint.decode(malformed)).isFailure,
        datetime.decode(datetime.encodeValue(java.time.Instant.ofEpochSecond(-9, 123))) == java.time.Instant
          .ofEpochSecond(-9, 123),
        Try(
          datetime.decode(WitSchemaValueTree(Vector(WitSchemaValueNode.DatetimeValue(Datetime(0, 1000000000))), 0))
        ).isFailure
      )
    },
    test("UUIDs use the native uuid node and match the schema-value path") {
      val nil    = new UUID(0L, 0L)
      val max    = UUID.fromString("ffffffff-ffff-ffff-ffff-ffffffffffff")
      val codec  = ConcreteCodec.derived[UuidHolder]
      val holder = UuidHolder(max, Some(nil), List(nil, max))
      val nested = codec.encode(holder)
      assertTrue(
        ConcreteCodec.uuid.graph == WitSchemaGraph(
          Vector(WitSchemaTypeNode(WitSchemaTypeBody.UuidType, MetadataEnvelope.empty)),
          Vector.empty,
          0
        ),
        ConcreteCodec.uuid.encodeValue(max) == WitSchemaValueTree(Vector(WitSchemaValueNode.UuidValue(max)), 0),
        ConcreteCodec.uuid.decode(ConcreteCodec.uuid.encodeValue(nil)) == nil,
        ConcreteCodec.uuid.decode(ConcreteCodec.uuid.encodeValue(max)) == max,
        SchemaWire.schemaGraphFromWit(ConcreteCodec.uuid.graph) == IntoSchema[UUID].graph,
        codec.decode(nested.value) == holder,
        SchemaWire.schemaGraphFromWit(nested.graph) == IntoSchema[UuidHolder].graph,
        SchemaWire.schemaValueFromWit(nested.value) == IntoSchema[UuidHolder].toValue(holder),
        FromSchema[UuidHolder].fromValue(SchemaWire.schemaValueFromWit(nested.value)) == Right(holder)
      )
    },
    test("sets use list carriers and decode with set semantics") {
      val codec      = ConcreteCodec.derived[Set[Int]]
      val nested     = ConcreteCodec.derived[Option[Set[Int]]]
      val value      = Set(3, 7)
      val duplicates = ConcreteCodec.derived[List[Int]].encodeValue(List(3, 7, 3))
      assertTrue(
        codec.decode(codec.encodeValue(value)) == value,
        codec.decode(codec.encodeValue(Set.empty)) == Set.empty[Int],
        codec.decode(duplicates) == value,
        nested.decode(nested.encodeValue(Some(value))) == Some(value),
        SchemaWire.schemaGraphFromWit(codec.graph) == IntoSchema[Set[Int]].graph,
        SchemaWire.schemaValueFromWit(codec.encodeValue(value)) == IntoSchema[Set[Int]].toValue(value)
      )
    },
    test("byte arrays use the HTTP contract's unsigned byte list") {
      val codec    = ConcreteCodec.derived[Array[Byte]]
      val value    = Array[Byte](0, -1, 127, -128)
      val encoded  = codec.encodeValue(value)
      val elements = encoded.valueNodes(encoded.root) match {
        case WitSchemaValueNode.ListValue(elements) => elements
        case _                                      => throw new AssertionError("expected byte list")
      }
      assertTrue(
        elements.map(encoded.valueNodes) == Vector(
          WitSchemaValueNode.U8Value(0),
          WitSchemaValueNode.U8Value(255),
          WitSchemaValueNode.U8Value(127),
          WitSchemaValueNode.U8Value(128)
        ),
        codec.decode(encoded).sameElements(value)
      )
    },
    test("decoding uses concrete structure, not a supplied graph") {
      val codec = ConcreteCodec.derived[(Int, Option[List[String]])]
      val good  =
        codec.encode((91, Some(List("yes", "no")))).copy(graph = WitSchemaGraph(Vector.empty, Vector.empty, -999))
      val wrong    = WitSchemaValueTree(Vector(WitSchemaValueNode.RecordValue(Vector.empty)), 0)
      val badIndex = WitSchemaValueTree(
        Vector(WitSchemaValueNode.TupleValue(Vector(77, 1)), WitSchemaValueNode.OptionValue(None)),
        0
      )
      assertTrue(
        codec.decode(good.value) == (91, Some(List("yes", "no"))),
        Try(codec.decode(wrong)).isFailure,
        Try(codec.decode(badIndex)).isFailure
      )
    },
    test("aliased and unreachable resources reject and release every handle") {
      val resource = ConcreteCodec.scalar[GuestSecretHandle](WitSchemaTypeBody.SecretType(WitSecretSpec(0, None)))(
        WitSchemaValueNode.SecretValue.apply
      ) { case WitSchemaValueNode.SecretValue(handle) => handle }
      val codec  = ConcreteCodec.list(resource)
      val handle = GuestSecretHandle.fromRaw(new Object)
      val alias  = WitSchemaValueTree(
        Vector(WitSchemaValueNode.SecretValue(handle), WitSchemaValueNode.ListValue(Vector(0, 0))),
        1
      )
      val aliased     = Try(codec.decode(alias))
      val unused      = GuestSecretHandle.fromRaw(new Object)
      val unreachable = WitSchemaValueTree(
        Vector(WitSchemaValueNode.SecretValue(unused), WitSchemaValueNode.ListValue(Vector.empty)),
        1
      )
      assertTrue(aliased.isFailure, !handle.isPresent, Try(codec.decode(unreachable)).isFailure, !unused.isPresent)
    },
    test("a later malformed field closes an already decoded stream without pulling it") {
      ZIO.fromFuture { implicit ec =>
        var pulls  = 0
        var closes = 0
        val source = AgentStream.fromPull[Int](
          () => { pulls += 1; Future.successful(Some(19)) },
          () => { closes += 1; Future.successful(()) }
        )
        val stream    = ConcreteCodec.derived[AgentStream[Int]].encodeValue(source)
        val codec     = ConcreteCodec.derived[(AgentStream[Int], String)]
        val malformed = WitSchemaValueTree(
          stream.valueNodes ++ Vector(
            WitSchemaValueNode.S32Value(42),
            WitSchemaValueNode.TupleValue(Vector(0, 1))
          ),
          2
        )
        val result = Try(codec.decode(malformed))
        Future.successful(assertTrue(result.isFailure, pulls == 0, closes == 1))
      }
    },
    test("a later output encoding failure rolls back a moved stream") {
      ZIO.fromFuture { implicit ec =>
        var closes = 0
        val source = AgentStream.fromPull[Int](
          () => Future.successful(None),
          () => { closes += 1; Future.successful(()) }
        )
        val codec  = ConcreteCodec.derived[(AgentStream[Int], golem.UInt)]
        val result = Try(codec.encodeValue((source, new golem.UInt(-1))))
        Future.successful(assertTrue(result.isFailure, closes == 1))
      }
    },
    test("unsigned values outside their range fail with the encode error") {
      assertTrue(
        encodeFailure(Try(ConcreteCodec.ulong.encodeValue(new golem.ULong(BigInt(1) << 64)))),
        encodeFailure(Try(ConcreteCodec.ulong.encodeValue(new golem.ULong(BigInt(-1))))),
        encodeFailure(Try(ConcreteCodec.uint.encodeValue(new golem.UInt(0x100000000L)))),
        encodeFailure(Try(ConcreteCodec.uint.encodeValue(new golem.UInt(-1)))),
        encodeFailure(Try(ConcreteCodec.ushort.encodeValue(new golem.UShort(65536)))),
        encodeFailure(Try(ConcreteCodec.ubyte.encodeValue(new golem.UByte(256.toShort)))),
        ConcreteCodec.ulong.decode(ConcreteCodec.ulong.encodeValue(new golem.ULong((BigInt(1) << 64) - 1))) ==
          new golem.ULong((BigInt(1) << 64) - 1)
      )
    },
    test("unparsable numbers and out-of-range time values fail with the schema errors") {
      val text    = WitSchemaValueTree(Vector(WitSchemaValueNode.StringValue("twelve")), 0)
      val instant = WitSchemaValueTree(Vector(WitSchemaValueNode.DatetimeValue(Datetime(Long.MaxValue, 0))), 0)
      assertTrue(
        decodeFailure(Try(ConcreteCodec.derived[BigInt].decode(text))),
        decodeFailure(Try(ConcreteCodec.derived[BigDecimal].decode(text))),
        decodeFailure(Try(ConcreteCodec.instant.decode(instant))),
        encodeFailure(Try(ConcreteCodec.duration.encodeValue(java.time.Duration.ofSeconds(Long.MaxValue)))),
        ConcreteCodec
          .derived[BigInt]
          .decode(ConcreteCodec.derived[BigInt].encodeValue(BigInt("-123456789012345678901"))) ==
          BigInt("-123456789012345678901")
      )
    },
    test("surrogate chars and strings with a lone surrogate fail with the encode error") {
      val emoji    = "a\ud83d\ude00b"
      val charNode = (value: Int) => WitSchemaValueTree(Vector(WitSchemaValueNode.CharValue(value)), 0)
      assertTrue(
        encodeFailure(Try(ConcreteCodec.char.encodeValue('\ud800'))),
        encodeFailure(Try(ConcreteCodec.char.encodeValue('\udfff'))),
        encodeFailure(Try(ConcreteCodec.string.encodeValue("a\ud800"))),
        encodeFailure(Try(ConcreteCodec.string.encodeValue("\udc00a"))),
        encodeFailure(Try(ConcreteCodec.string.encodeValue("\ude00\ud83d"))),
        encodeFailure(Try(ConcreteCodec.derived[Item].encodeValue(Item("\ud800", List(Left("ok")), None)))),
        encodeFailure(Try(ConcreteCodec.derived[Item].encodeValue(Item("ok", List(Left("\udfff")), None)))),
        encodeFailure(Try(IntoSchema[Item].toValue(Item("\ud800", Nil, None)))),
        encodeFailure(Try(IntoSchema[Char].toValue('\ud800'))),
        decodeFailure(Try(ConcreteCodec.char.decode(charNode(0xd800)))),
        decodeFailure(Try(ConcreteCodec.char.decode(charNode(0x1f600)))),
        FromSchema[Char].fromValue(SchemaValue.CharValue(0xdfff)).isLeft,
        ConcreteCodec.string.decode(ConcreteCodec.string.encodeValue(emoji)) == emoji,
        ConcreteCodec.char.decode(ConcreteCodec.char.encodeValue('\uffff')) == '\uffff',
        IntoSchema[String].toValue(emoji) == SchemaValue.StringValue(emoji),
        UnicodeText.loneSurrogateIndex(emoji) == -1,
        UnicodeText.loneSurrogateIndex("ab\ud83d") == 2
      )
    },
    test("textual leaves with a lone surrogate fail with the encode error") {
      val text   = UnstructuredText.concreteCodec[String]
      val binary = UnstructuredBinary.concreteCodec[String]
      assertTrue(
        encodeFailure(Try(ConcreteCodec.path.encodeValue(GolemPath("a\ud800")))),
        uriEncodeFailure("\udfff")(ConcreteCodec.url.encodeValue),
        encodeFailure(Try(IntoSchema[GolemPath].toValue(GolemPath("a\ud800")))),
        uriEncodeFailure("\udfff")(IntoSchema[URI].toValue),
        encodeFailure(Try(IntoSchema[Located].toValue(Located(GolemPath("\ud800"), new URI("ok"))))),
        uriEncodeFailure("\udc00")(uri => IntoSchema[Located].toValue(Located(GolemPath("ok"), uri))),
        encodeFailure(Try(text.encodeValue(UnstructuredText.fromInline[String]("a\ud800")))),
        encodeFailure(Try(text.encodeValue(UnstructuredText.fromInline[String]("ok", "\ud800")))),
        encodeFailure(Try(text.encodeValue(UnstructuredText.fromUrl[String]("\udfff")))),
        encodeFailure(Try(IntoSchema[UnstructuredText[String]].toValue(UnstructuredText.fromInline[String]("\ud800")))),
        encodeFailure(Try(binary.encodeValue(UnstructuredBinary.fromInline[String](Array[Byte](1), "\ud800")))),
        ConcreteCodec.url.decode(ConcreteCodec.url.encodeValue(new URI("https://example.com/\ud83d\ude00"))) ==
          new URI("https://example.com/\ud83d\ude00"),
        text.decode(text.encodeValue(UnstructuredText.fromInline[String]("\ud83d\ude00", "en"))) ==
          UnstructuredText.fromInline[String]("\ud83d\ude00", "en")
      )
    },
    test("nested sealed traits are nested variants on both paths") {
      val codec  = ConcreteCodec.derived[Animal]
      val values = List[Animal](Dog("rex"), Cat, Wild("wolf"))
      assertTrue(
        SchemaWire.schemaGraphFromWit(codec.graph) == IntoSchema[Animal].graph,
        values.forall { value =>
          val encoded = codec.encodeValue(value)
          codec.decode(encoded) == value &&
          SchemaWire.schemaValueFromWit(encoded) == IntoSchema[Animal].toValue(value) &&
          FromSchema[Animal].fromValue(SchemaWire.schemaValueFromWit(encoded)) == Right(value)
        }
      )
    },
    test("tuples over 22 elements are flat tuples on both paths") {
      val codec       = ConcreteCodec.derived[Wide]
      val value: Wide = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, "last")
      val encoded     = codec.encodeValue(value)
      assertTrue(
        SchemaWire.schemaGraphFromWit(codec.graph) == IntoSchema[Wide].graph,
        IntoSchema[Wide].graph.root.body == SchemaTypeBody.TupleType(List.fill(22)(t.s32) :+ t.string),
        codec.decode(encoded) == value,
        SchemaWire.schemaValueFromWit(encoded) == IntoSchema[Wide].toValue(value),
        FromSchema[Wide].fromValue(SchemaWire.schemaValueFromWit(encoded)) == Right(value)
      )
    },
    test("nested Instant and Duration fields use the zio-blocks primitives on the schema path") {
      val codec = ConcreteCodec.derived[Timed]
      val value = Timed(
        java.time.Instant.ofEpochSecond(-5, 7),
        java.time.Duration.ofMillis(1500),
        List(java.time.Instant.EPOCH)
      )
      val encoded = codec.encodeValue(value)
      assertTrue(
        SchemaWire.schemaGraphFromWit(codec.graph) == IntoSchema[Timed].graph,
        codec.decode(encoded) == value,
        SchemaWire.schemaValueFromWit(encoded) == IntoSchema[Timed].toValue(value),
        FromSchema[Timed].fromValue(SchemaWire.schemaValueFromWit(encoded)) == Right(value)
      )
    },
    test("collection subtypes without a concrete carrier do not compile") {
      assertTrue(
        scala.compiletime.testing
          .typeCheckErrors("golem.schema.wire.ConcreteCodec.derived[scala.collection.immutable.ListMap[String, Int]]")
          .exists(_.message.contains("ListMap")),
        scala.compiletime.testing
          .typeCheckErrors("golem.schema.wire.ConcreteCodec.derived[scala.collection.immutable.SortedSet[Int]]")
          .exists(_.message.contains("SortedSet")),
        scala.compiletime.testing
          .typeCheckErrors("golem.schema.wire.ConcreteCodec.derived[IndexedSeq[Int]]")
          .exists(_.message.contains("IndexedSeq"))
      )
    },
    test("recursive wire nodes and wrong variant payloads reject") {
      val item   = ConcreteCodec.derived[Item]
      val cyclic = WitSchemaValueTree(
        Vector(
          WitSchemaValueNode.StringValue("loop"),
          WitSchemaValueNode.ListValue(Vector.empty),
          WitSchemaValueNode.OptionValue(Some(3)),
          WitSchemaValueNode.RecordValue(Vector(0, 1, 2))
        ),
        3
      )
      val choice  = ConcreteCodec.derived[Choice]
      val invalid = WitSchemaValueTree(Vector(WitSchemaValueNode.VariantValue(WitVariantValuePayload(1, None))), 0)
      assertTrue(Try(item.decode(cyclic)).isFailure, Try(choice.decode(invalid)).isFailure)
    }
  )
}
