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

package golem.tools

import golem.data._
import golem.runtime.{AgentMetadata, MethodMetadata}
import zio.test._

object SchemaJsonEncoderSpec extends ZIOSpecDefault {
  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaJsonEncoderSpec")(
      test("encodes tuple schemas with component elements") {
        val schema = StructuredSchema.Tuple(
          List(
            NamedElementSchema("name", ElementSchema.Component(DataType.StringType)),
            NamedElementSchema("age", ElementSchema.Component(DataType.IntType))
          )
        )
        val encoded = SchemaJsonEncoder.encode(schema)

        assertTrue(
          encoded.obj("tag").str == "tuple",
          encoded.obj("val").arr.length == 2
        )
      },
      test("encodes multimodal schemas with text and binary restrictions") {
        val schema = StructuredSchema.Multimodal(
          List(
            NamedElementSchema("text", ElementSchema.UnstructuredText(Some(List("en", "es")))),
            NamedElementSchema("image", ElementSchema.UnstructuredBinary(None))
          )
        )
        val encoded  = SchemaJsonEncoder.encode(schema)
        val elements = encoded.obj("val").arr

        assertTrue(
          encoded.obj("tag").str == "multimodal",
          elements.length == 2,
          elements.head.arr(1).obj("tag").str == "unstructured-text",
          elements(1).arr(1).obj("tag").str == "unstructured-binary"
        )
      },
      test("encodes agent metadata with methods") {
        val inputSchema  = StructuredSchema.single(ElementSchema.Component(DataType.StringType))
        val outputSchema = StructuredSchema.single(ElementSchema.Component(DataType.BoolType))

        val method = MethodMetadata(
          name = "ping",
          description = Some("check connectivity"),
          prompt = Some("reply true if reachable"),
          mode = None,
          input = inputSchema,
          output = outputSchema
        )
        val metadata = AgentMetadata(
          name = "HealthAgent",
          description = Some("health checks"),
          mode = None,
          methods = List(method),
          constructor = StructuredSchema.Tuple(Nil)
        )

        val encoded = AgentTypeJsonEncoder.encode("HealthAgent", metadata)
        val methods = encoded.obj("methods").arr

        assertTrue(
          encoded.obj("name").str == "HealthAgent",
          methods.length == 1,
          methods.head.obj("name").str == "ping",
          methods.head.obj("prompt").str == "reply true if reachable"
        )
      },
      test("encodes tuple, list, option, record, and variant component types") {
        val tupleType  = DataType.TupleType(List(DataType.IntType, DataType.StringType))
        val listType   = DataType.ListType(DataType.BoolType)
        val optType    = DataType.Optional(DataType.DoubleType)
        val recordType =
          DataType.StructType(
            List(
              DataType.Field("id", DataType.IntType, optional = false),
              DataType.Field("name", DataType.StringType, optional = false)
            )
          )
        val variantType =
          DataType.EnumType(
            List(
              DataType.EnumCase("On", None),
              DataType.EnumCase("Off", Some(DataType.StringType))
            )
          )

        val schema = StructuredSchema.Tuple(
          List(
            NamedElementSchema("tuple", ElementSchema.Component(tupleType)),
            NamedElementSchema("list", ElementSchema.Component(listType)),
            NamedElementSchema("opt", ElementSchema.Component(optType)),
            NamedElementSchema("record", ElementSchema.Component(recordType)),
            NamedElementSchema("variant", ElementSchema.Component(variantType))
          )
        )

        val encoded = SchemaJsonEncoder.encode(schema)
        val nodes   =
          encoded
            .obj("val")
            .arr
            .flatMap { element =>
              element.arr(1).obj("val").obj("nodes").arr.map(_.obj("type").obj("tag").str)
            }
            .toSet

        assertTrue(
          nodes("tuple-type"),
          nodes("list-type"),
          nodes("option-type"),
          nodes("record-type"),
          nodes("variant-type")
        )
      },
      test("encodes map types as list of key/value entries") {
        val schema = StructuredSchema.single(
          ElementSchema.Component(DataType.MapType(DataType.StringType, DataType.StringType))
        )
        val encoded = SchemaJsonEncoder.encode(schema)
        val nodes   = encoded
          .obj("val")
          .arr(0)
          .arr(1)
          .obj("val")
          .obj("nodes")
          .arr
          .map(_.obj("type").obj("tag").str)
          .toSet

        assertTrue(
          nodes("list-type"),
          nodes("record-type")
        )
      },
      test("encodes unstructured text and binary restrictions") {
        val schema = StructuredSchema.Multimodal(
          List(
            NamedElementSchema("textSome", ElementSchema.UnstructuredText(Some(List("en")))),
            NamedElementSchema("textNone", ElementSchema.UnstructuredText(None)),
            NamedElementSchema("binSome", ElementSchema.UnstructuredBinary(Some(List("image/png")))),
            NamedElementSchema("binNone", ElementSchema.UnstructuredBinary(None))
          )
        )
        val encoded  = SchemaJsonEncoder.encode(schema)
        val elements = encoded.obj("val").arr

        assertTrue(
          elements(0).arr(1).obj("tag").str == "unstructured-text",
          elements(0).arr(1).obj("val").obj("tag").str == "some",
          elements(1).arr(1).obj("val").obj("tag").str == "none",
          elements(2).arr(1).obj("tag").str == "unstructured-binary",
          elements(2).arr(1).obj("val").obj("tag").str == "some",
          elements(3).arr(1).obj("val").obj("tag").str == "none"
        )
      },
      test("encodes primitive and bytes component types") {
        val schema = StructuredSchema.Tuple(
          List(
            NamedElementSchema("unit", ElementSchema.Component(DataType.UnitType)),
            NamedElementSchema("bool", ElementSchema.Component(DataType.BoolType)),
            NamedElementSchema("string", ElementSchema.Component(DataType.StringType)),
            NamedElementSchema("long", ElementSchema.Component(DataType.LongType)),
            NamedElementSchema("double", ElementSchema.Component(DataType.DoubleType)),
            NamedElementSchema("big", ElementSchema.Component(DataType.BigDecimalType)),
            NamedElementSchema("uuid", ElementSchema.Component(DataType.UUIDType)),
            NamedElementSchema("bytes", ElementSchema.Component(DataType.BytesType))
          )
        )
        val encoded = SchemaJsonEncoder.encode(schema)
        val tags    =
          encoded
            .obj("val")
            .arr
            .flatMap(_.arr(1).obj("val").obj("nodes").arr.map(_.obj("type").obj("tag").str))
            .toSet

        assertTrue(
          tags("prim-bool-type"),
          tags("prim-string-type"),
          tags("prim-s64-type"),
          tags("prim-f64-type"),
          tags("tuple-type"),
          tags("list-type")
        )
      }
    )
}
