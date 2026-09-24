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
import zio.ZIO
import zio.blocks.schema.Schema
import zio.test.*

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
