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

package golem.tool

import golem.schema._
import zio.test._

import scala.collection.immutable.ListMap

object ToolLiteralSpec extends ZIOSpecDefault {

  private def graph(root: SchemaType): SchemaGraph = SchemaGraph(ListMap.empty, root)

  def spec: Spec[Any, Any] = suite("ToolLiteralSpec")(
    test("string_literal") {
      val v = ToolLiterals.literalToSchemaValue(graph(t.string), ToolLiteral.StrLiteral("hi")).toOption.get
      assertTrue(v == SchemaValue.StringValue("hi"))
    },
    test("enum_case_by_name") {
      val enumTy = t.`enum`(List("always", "never", "auto"))
      val v      = ToolLiterals.literalToSchemaValue(graph(enumTy), ToolLiteral.StrLiteral("auto")).toOption.get
      assertTrue(v == SchemaValue.EnumValue(2))
    },
    test("unknown_enum_case_errors") {
      val enumTy = t.`enum`(List("always"))
      val err    = ToolLiterals.literalToSchemaValue(graph(enumTy), ToolLiteral.StrLiteral("nope"))
      assertTrue(err.left.toOption.exists(_.isInstanceOf[ToolBuildError.DefaultTypeMismatch]))
    },
    test("u64_max_in_range") {
      val v = ToolLiterals
        .literalToSchemaValue(graph(t.u64), ToolLiteral.IntLiteral((BigInt(1) << 64) - 1))
        .toOption
        .get
      assertTrue(v == SchemaValue.U64Value(-1L))
    },
    test("integer_out_of_range_errors") {
      val err = ToolLiterals.literalToSchemaValue(graph(t.u32), ToolLiteral.IntLiteral(BigInt(-1)))
      assertTrue(err.left.toOption.exists(_.isInstanceOf[ToolBuildError.DefaultTypeMismatch]))
    },
    test("path_literal") {
      val v = ToolLiterals
        .literalToSchemaValue(
          graph(SchemaType(SchemaTypeBody.PathType(PathSpec(PathDirection.InOut, PathKind.Any)))),
          ToolLiteral.StrLiteral(".git")
        )
        .toOption
        .get
      assertTrue(v == SchemaValue.PathValue(".git"))
    },
    test("list_of_strings") {
      val v = ToolLiterals
        .literalToSchemaValue(
          graph(t.list(t.string)),
          ToolLiteral.ListLiteral(List(ToolLiteral.StrLiteral("a"), ToolLiteral.StrLiteral("b")))
        )
        .toOption
        .get
      assertTrue(v == SchemaValue.ListValue(List(SchemaValue.StringValue("a"), SchemaValue.StringValue("b"))))
    },
    test("map_of_strings") {
      val v = ToolLiterals
        .literalToSchemaValue(
          graph(t.map(t.string, t.string)),
          ToolLiteral.MapLiteral(List((ToolLiteral.StrLiteral("k"), ToolLiteral.StrLiteral("v"))))
        )
        .toOption
        .get
      assertTrue(
        v == SchemaValue.MapValue(List(SchemaMapEntry(SchemaValue.StringValue("k"), SchemaValue.StringValue("v"))))
      )
    },
    test("fixed_list_of_matching_length") {
      val v = ToolLiterals
        .literalToSchemaValue(
          graph(t.fixedList(t.u32, 2)),
          ToolLiteral.ListLiteral(List(ToolLiteral.IntLiteral(BigInt(1)), ToolLiteral.IntLiteral(BigInt(2))))
        )
        .toOption
        .get
      assertTrue(v == SchemaValue.FixedListValue(List(SchemaValue.U32Value(1), SchemaValue.U32Value(2))))
    },
    test("fixed_list_of_wrong_length_is_rejected") {
      val err = ToolLiterals.literalToSchemaValue(
        graph(t.fixedList(t.u32, 2)),
        ToolLiteral.ListLiteral(List(ToolLiteral.IntLiteral(BigInt(1))))
      )
      assertTrue(err.isLeft)
    }
  )
}
