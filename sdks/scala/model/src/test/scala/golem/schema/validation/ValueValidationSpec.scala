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
import golem.schema.SchemaValue._
import golem.schema.validation.ValueError._
import zio.test._

import scala.collection.immutable.ListMap

object ValueValidationSpec extends ZIOSpecDefault {
  private def graph(root: SchemaType, defs: ListMap[String, SchemaTypeDef] = ListMap.empty)                       = SchemaGraph(defs, root)
  private def validate(tpe: SchemaType, value: SchemaValue, defs: ListMap[String, SchemaTypeDef] = ListMap.empty) =
    ValueValidation.validateValue(graph(tpe, defs), tpe, value)
  override def spec = suite("ValueValidationSpec")(
    test("primitive shape mismatch is reported") {
      assertTrue(validate(t.s32, S64Value(1)).left.exists(_.head.isInstanceOf[ShapeMismatch]))
    },
    test("variant case out of range is reported") {
      assertTrue(
        validate(t.variant(List(t.variantCase("a"))), VariantValue(7, None)).left
          .exists(_.head.isInstanceOf[VariantCaseOutOfRange])
      )
    },
    test("enum case out of range is reported") {
      assertTrue(validate(t.`enum`(List("a", "b")), EnumValue(5)).left.exists(_.head.isInstanceOf[EnumCaseOutOfRange]))
    },
    test("record arity mismatch is reported") {
      assertTrue(
        validate(t.record(List(t.field("a", t.bool), t.field("b", t.bool))), RecordValue(List(BoolValue(true)))).left
          .exists(_.head.isInstanceOf[RecordArityMismatch])
      )
    },
    test("fixed list length mismatch is reported") {
      assertTrue(
        validate(t.fixedList(t.bool, 3), FixedListValue(List(BoolValue(true)))).left
          .exists(_.head.isInstanceOf[FixedListLengthMismatch])
      )
    },
    test("union unknown tag is reported") {
      val ty = SchemaType(UnionType(List(UnionBranch("x", t.string, DiscriminatorRule.Prefix("")))));
      assertTrue(
        validate(ty, UnionValue("nope", StringValue("anything"))).left.exists(_.head.isInstanceOf[UnionUnknownTag])
      )
    },
    test("union discriminator mismatch is reported") {
      val ty = SchemaType(UnionType(List(UnionBranch("x", t.string, DiscriminatorRule.Prefix("https://")))));
      assertTrue(
        validate(ty, UnionValue("x", StringValue("ftp://x"))).left
          .exists(_.exists(_.isInstanceOf[UnionDiscriminatorMismatch]))
      )
    },
    test("direct ref cycle returns recursive ref error") {
      val g = graph(t.ref("A"), ListMap("A" -> SchemaTypeDef(t.ref("A"))));
      assertTrue(
        ValueValidation.validateValue(g, g.root, BoolValue(true)).left.exists(_.exists(_.isInstanceOf[RecursiveRef]))
      )
    },
    test("full length alias chain validates") {
      val defs = (0 until 8).foldLeft(ListMap.empty[String, SchemaTypeDef])((m, i) =>
        m + (s"a$i" -> SchemaTypeDef(if (i < 7) t.ref(s"a${i + 1}") else t.bool))
      );
      val g = graph(t.ref("a0"), defs); assertTrue(ValueValidation.validateValue(g, g.root, BoolValue(true)).isRight)
    },
    test("url host allow list rejects unlisted") {
      val ty = SchemaType(UrlType(UrlRestrictions(allowedHosts = Some(List("example.com")))));
      assertTrue(
        validate(ty, UrlValue("https://attacker.com/")).left.exists(_.exists(_.isInstanceOf[UrlHostNotAllowed]))
      )
    },
    test("numeric below min is rejected") {
      val ty = SchemaType(U32Type(NumericRestrictions(min = Some(NumericBound.Unsigned(1))).normalize));
      assertTrue(validate(ty, U32Value(0)).left.exists(_.exists(_.isInstanceOf[NumericOutOfRange])))
    }
  )
}
