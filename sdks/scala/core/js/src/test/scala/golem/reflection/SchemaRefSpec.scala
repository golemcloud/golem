/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import golem.schema._
import golem.schema.SchemaTypeBody._
import golem.schema.SchemaValue._
import zio.test._
import zio.blocks.schema.json.Json

import scala.collection.immutable.ListMap

object SchemaRefSpec extends ZIOSpecDefault {
  private val graph = SchemaGraph(
    ListMap.empty,
    SchemaType(
      RecordType(
        List(
          NamedFieldType("name", SchemaType(StringType)),
          NamedFieldType("count", SchemaType(U32Type())),
          NamedFieldType("enabled", SchemaType(BoolType)),
          NamedFieldType("labels", SchemaType(ListType(SchemaType(StringType))))
        )
      )
    )
  )

  def spec = suite("SchemaRef")(
    test("packs and unpacks canonical record JSON") {
      val ref  = SchemaRef(graph)
      val json = Json.Object(
        "name"    -> Json.String("worker"),
        "count"   -> Json.Number(BigDecimal(42)),
        "enabled" -> Json.Boolean(true),
        "labels"  -> Json.Array(Json.String("a"), Json.String("b"))
      )
      val expected = RecordValue(
        List(StringValue("worker"), U32Value(42), BoolValue(true), ListValue(List(StringValue("a"), StringValue("b"))))
      )
      assertTrue(ref.packJson(json) == Right(expected), ref.unpackJson(expected) == Right(json))
    },
    test("rejects unknown fields and invalid direct values") {
      val ref         = SchemaRef(graph)
      val invalidJson = Json.Object(
        "name"    -> Json.String("worker"),
        "count"   -> Json.Number(BigDecimal(1)),
        "enabled" -> Json.Boolean(true),
        "labels"  -> Json.Array(),
        "extra"   -> Json.String("no")
      )
      assertTrue(
        ref.packJson(invalidJson).isLeft,
        ref.validateValue(RecordValue(List(StringValue("too-short")))).isLeft
      )
    },
    test("round-trips rich canonical JSON values") {
      val rich = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(
            TupleType(
              List(
                SchemaType(BinaryType(BinaryRestrictions.empty)),
                SchemaType(DatetimeType),
                SchemaType(DurationType)
              )
            )
          )
        )
      )
      val value = TupleValue(
        List(
          BinaryValue(Vector(0, 1, 2, -1), Some("application/octet-stream")),
          DatetimeValue(Datetime(1704067200L, 123000000)),
          DurationValue(3723000000004L)
        )
      )
      assertTrue(rich.unpackJson(value).flatMap(rich.packJson) == Right(value))
    },
    test("does not expose capabilities as JSON") {
      val ref = SchemaRef(
        SchemaGraph(ListMap.empty, SchemaType(PermissionCardType(PermissionCardSpec(polymorphic = false))))
      )
      assertTrue(ref.packJson(Json.Null).isLeft)
    },
    test("renders canonical JSON Schema") {
      val rendered = SchemaRef(graph).toJsonSchema()
      assertTrue(
        rendered.get("$schema").one == Right(Json.String("https://json-schema.org/draft/2020-12/schema")),
        rendered.get("type").one == Right(Json.String("object"))
      )
    }
  )
}
