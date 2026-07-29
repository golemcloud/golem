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

package golem.schema.validation

import golem.schema._
import golem.schema.SchemaTypeBody._
import golem.schema.validation.SchemaError._
import zio.test._

import scala.collection.immutable.ListMap

object WellFormednessSpec extends ZIOSpecDefault {
  private def graph(root: SchemaType, defs: ListMap[String, SchemaTypeDef] = ListMap.empty) = SchemaGraph(defs, root)
  override def spec                                                                         = suite("WellFormednessSpec")(
    test("dangling ref is reported") {
      assertTrue(WellFormedness.validateGraph(graph(t.ref("missing"))).left.exists(_.contains(DanglingRef("missing"))))
    },
    test("pure recursive alias is rejected") {
      val g = graph(t.ref("A"), ListMap("A" -> SchemaTypeDef(t.ref("A"))))
      assertTrue(WellFormedness.validateGraph(g).left.exists(_.exists(_.isInstanceOf[RecursiveAlias])))
    },
    test("legitimate recursive type through constructor is accepted") {
      val g = graph(
        t.ref("Tree"),
        ListMap("Tree" -> SchemaTypeDef(t.record(List(t.field("children", t.list(t.ref("Tree")))))))
      )
      assertTrue(WellFormedness.validateGraph(g).isRight)
    },
    test("duplicate field is reported") {
      val g = graph(t.record(List(t.field("a", t.bool), t.field("a", t.s32))))
      assertTrue(WellFormedness.validateGraph(g).left.exists(_.contains(DuplicateFieldName("a"))))
    },
    test("map key not primitive is reported") {
      val g = graph(t.map(t.record(Nil), t.bool))
      assertTrue(WellFormedness.validateGraph(g).left.exists(_.contains(MapKeyNotPrimitive)))
    },
    test("fixed list zero length is reported") {
      assertTrue(
        WellFormedness.validateGraph(graph(t.fixedList(t.bool, 0))).left.exists(_.contains(FixedListZeroLength))
      )
    },
    test("quantity min greater than max is reported") {
      val q = SchemaType(
        QuantityType(QuantitySpec("kg", Nil, Some(QuantityValue(10, 0, "kg")), Some(QuantityValue(1, 0, "kg"))))
      )
      assertTrue(WellFormedness.validateGraph(graph(q)).left.exists(_.contains(QuantityMinGreaterThanMax)))
    },
    test("union string rule on record body is reported") {
      val u = SchemaType(UnionType(List(UnionBranch("t", t.record(Nil), DiscriminatorRule.Prefix("x")))))
      assertTrue(WellFormedness.validateGraph(graph(u)).left.exists(_.contains(UnionStringRuleOnNonStringBody("t"))))
    },
    test("duplicate union tag is reported") {
      val u = SchemaType(
        UnionType(
          List(
            UnionBranch("x", t.string, DiscriminatorRule.Prefix("a")),
            UnionBranch("x", t.string, DiscriminatorRule.Prefix("b"))
          )
        )
      )
      assertTrue(WellFormedness.validateGraph(graph(u)).left.exists(_.contains(DuplicateUnionTag("x"))))
    },
    test("nested option is rejected") {
      assertTrue(
        WellFormedness
          .validateGraph(graph(t.option(t.option(t.u32))))
          .left
          .exists(_.exists(_.isInstanceOf[NullableNesting]))
      )
    }
  )
}
