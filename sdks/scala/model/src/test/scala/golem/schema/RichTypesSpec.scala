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

import golem.schema.Implicits._
import zio.blocks.schema.Schema
import zio.blocks.typeid.TypeId
import zio.test._

import java.time.{Duration => JDuration, Instant}

object RichTypesSpec extends ZIOSpecDefault {
  sealed trait Bytes
  sealed trait Seconds

  implicit val bytesUnit: QuantityUnit[Bytes] = new QuantityUnit[Bytes] {
    override val baseUnit: String              = "bytes"
    override val allowedSuffixes: List[String] = List("kb", "mb")
    override val typeId: TypeId[Bytes]         = TypeId.of[Bytes]
  }

  implicit val secondsUnit: QuantityUnit[Seconds] = new QuantityUnit[Seconds] {
    override val baseUnit: String              = "seconds"
    override val allowedSuffixes: List[String] = List("ms")
    override val typeId: TypeId[Seconds]       = TypeId.of[Seconds]
  }

  final case class RichRecord(path: GolemPath, url: Url, at: Instant, duration: JDuration, size: Quantity[Bytes])

  object RichRecord {
    implicit val schema: Schema[RichRecord] = Schema.derived
  }

  final case class TwoQuantities(size: Quantity[Bytes], timeout: Quantity[Seconds])

  object TwoQuantities {
    implicit val schema: Schema[TwoQuantities] = Schema.derived
  }

  final case class SizeOnly(size: Quantity[Bytes])

  object SizeOnly {
    implicit val schema: Schema[SizeOnly] = Schema.derived
  }

  private def recordFieldBodies[A](implicit into: IntoSchema[A]): List[SchemaTypeBody] =
    into.graph.defs.values.head.body.body match {
      case SchemaTypeBody.RecordType(fields) => fields.map(_.body.body)
      case other                             => throw new AssertionError(s"expected record, got $other")
    }

  override def spec: Spec[TestEnvironment, Any] = suite("rich semantic types")(
    test("top-level schemas use rich nodes") {
      assertTrue(
        IntoSchema[GolemPath].graph.root.body == SchemaTypeBody.PathType(GolemPath.defaultSpec),
        IntoSchema[Url].graph.root.body == SchemaTypeBody.UrlType(Url.defaultRestrictions),
        IntoSchema[Instant].graph.root.body == SchemaTypeBody.DatetimeType,
        IntoSchema[JDuration].graph.root.body == SchemaTypeBody.DurationType,
        IntoSchema[Quantity[Bytes]].graph.root.body == SchemaTypeBody.QuantityType(bytesUnit.spec)
      )
    },
    test("top-level values roundtrip") {
      val path     = GolemPath("/tmp/in.txt")
      val url      = Url("https://example.com/a")
      val instant  = Instant.ofEpochSecond(1234L, 567)
      val duration = JDuration.ofSeconds(2L).plusNanos(3L)
      val quantity = Quantity[Bytes](42L, 1, "kb")
      assertTrue(
        FromSchema[GolemPath].fromValue(IntoSchema[GolemPath].toValue(path)) == Right(path),
        FromSchema[Url].fromValue(IntoSchema[Url].toValue(url)) == Right(url),
        FromSchema[Instant].fromValue(IntoSchema[Instant].toValue(instant)) == Right(instant),
        FromSchema[JDuration].fromValue(IntoSchema[JDuration].toValue(duration)) == Right(duration),
        FromSchema[Quantity[Bytes]].fromValue(IntoSchema[Quantity[Bytes]].toValue(quantity)) == Right(quantity)
      )
    },
    test("derived record fields use rich nodes and roundtrip") {
      val record = RichRecord(
        GolemPath("/workspace/data"),
        Url("https://golem.cloud"),
        Instant.ofEpochSecond(999L, 123456789),
        JDuration.ofNanos(-17L),
        Quantity[Bytes](1L, 0, "bytes")
      )
      val fieldBodies = recordFieldBodies[RichRecord]
      assertTrue(
        fieldBodies == List(
          SchemaTypeBody.PathType(GolemPath.defaultSpec),
          SchemaTypeBody.UrlType(Url.defaultRestrictions),
          SchemaTypeBody.DatetimeType,
          SchemaTypeBody.DurationType,
          SchemaTypeBody.QuantityType(bytesUnit.spec)
        ),
        FromSchema[RichRecord].fromValue(IntoSchema[RichRecord].toValue(record)) == Right(record)
      )
    },
    test("derived record supports two distinct quantity units") {
      val record = TwoQuantities(Quantity[Bytes](128L, 0, "kb"), Quantity[Seconds](30L, 0, "seconds"))
      assertTrue(
        recordFieldBodies[TwoQuantities] == List(
          SchemaTypeBody.QuantityType(bytesUnit.spec),
          SchemaTypeBody.QuantityType(secondsUnit.spec)
        ),
        FromSchema[TwoQuantities].fromValue(IntoSchema[TwoQuantities].toValue(record)) == Right(record)
      )
    },
    test("quantity schemas do not depend on global unit lifecycle") {
      val beforeSchema = IntoSchema[SizeOnly].graph
      val beforeValue  = IntoSchema[SizeOnly].toValue(SizeOnly(Quantity[Bytes](1L, 0, "bytes")))

      val twoUnitSchema = IntoSchema[TwoQuantities].graph
      val afterSchema   = IntoSchema[SizeOnly].graph
      val afterValue    = IntoSchema[SizeOnly].toValue(SizeOnly(Quantity[Bytes](1L, 0, "bytes")))

      assertTrue(
        beforeSchema == afterSchema,
        beforeValue == afterValue,
        twoUnitSchema.defs.values.head.body.body match {
          case SchemaTypeBody.RecordType(fields) =>
            fields.map(_.body.body) == List(
              SchemaTypeBody.QuantityType(bytesUnit.spec),
              SchemaTypeBody.QuantityType(secondsUnit.spec)
            )
          case _ => false
        }
      )
    },
    test("quantity rejects disallowed units") {
      val accepted = FromSchema[Quantity[Bytes]].fromValue(SchemaValue.QuantityValueNode(QuantityValue(1L, 0, "mb")))
      val rejected =
        FromSchema[Quantity[Bytes]].fromValue(SchemaValue.QuantityValueNode(QuantityValue(1L, 0, "seconds")))
      assertTrue(accepted == Right(Quantity[Bytes](1L, 0, "mb")), rejected.isLeft)
    }
  )
}
