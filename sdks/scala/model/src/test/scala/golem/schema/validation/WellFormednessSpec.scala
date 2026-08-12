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
  private sealed trait ExpectedClassification
  private case object Reject        extends ExpectedClassification
  private case object Disjoint      extends ExpectedClassification
  private case object Indeterminate extends ExpectedClassification
  private final case class ClassificationCase(
    name: String,
    left: DiscriminatorRule,
    right: DiscriminatorRule,
    expected: ExpectedClassification
  )
  private def fieldEquals(name: String, literal: Option[String]) =
    DiscriminatorRule.FieldEquals(FieldDiscriminator(name, literal))
  private val classificationCases = List(
    ClassificationCase(
      "prefix_prefix_nested_reject",
      DiscriminatorRule.Prefix("a"),
      DiscriminatorRule.Prefix("ab"),
      Reject
    ),
    ClassificationCase(
      "prefix_prefix_disjoint",
      DiscriminatorRule.Prefix("a"),
      DiscriminatorRule.Prefix("b"),
      Disjoint
    ),
    ClassificationCase(
      "empty_prefix_prefix_reject",
      DiscriminatorRule.Prefix(""),
      DiscriminatorRule.Prefix("a"),
      Reject
    ),
    ClassificationCase(
      "suffix_suffix_nested_reject",
      DiscriminatorRule.Suffix("ing"),
      DiscriminatorRule.Suffix("ng"),
      Reject
    ),
    ClassificationCase(
      "suffix_suffix_disjoint",
      DiscriminatorRule.Suffix("a"),
      DiscriminatorRule.Suffix("b"),
      Disjoint
    ),
    ClassificationCase(
      "empty_suffix_suffix_reject",
      DiscriminatorRule.Suffix(""),
      DiscriminatorRule.Suffix("a"),
      Reject
    ),
    ClassificationCase("prefix_suffix_reject", DiscriminatorRule.Prefix("a"), DiscriminatorRule.Suffix("b"), Reject),
    ClassificationCase(
      "prefix_contains_reject",
      DiscriminatorRule.Prefix("a"),
      DiscriminatorRule.Contains("b"),
      Reject
    ),
    ClassificationCase(
      "suffix_contains_reject",
      DiscriminatorRule.Suffix("a"),
      DiscriminatorRule.Contains("b"),
      Reject
    ),
    ClassificationCase(
      "contains_contains_reject",
      DiscriminatorRule.Contains("a"),
      DiscriminatorRule.Contains("b"),
      Reject
    ),
    ClassificationCase(
      "regex_regex_identical_reject",
      DiscriminatorRule.Regex("a.*"),
      DiscriminatorRule.Regex("a.*"),
      Reject
    ),
    ClassificationCase(
      "regex_regex_distinct_indeterminate",
      DiscriminatorRule.Regex("a.*"),
      DiscriminatorRule.Regex(".*a"),
      Indeterminate
    ),
    ClassificationCase(
      "regex_prefix_indeterminate",
      DiscriminatorRule.Regex("^a"),
      DiscriminatorRule.Prefix("a"),
      Indeterminate
    ),
    ClassificationCase(
      "regex_empty_prefix_indeterminate",
      DiscriminatorRule.Regex("^a"),
      DiscriminatorRule.Prefix(""),
      Indeterminate
    ),
    ClassificationCase(
      "regex_suffix_indeterminate",
      DiscriminatorRule.Regex("a$"),
      DiscriminatorRule.Suffix("a"),
      Indeterminate
    ),
    ClassificationCase(
      "regex_empty_suffix_indeterminate",
      DiscriminatorRule.Regex("a$"),
      DiscriminatorRule.Suffix(""),
      Indeterminate
    ),
    ClassificationCase(
      "regex_contains_indeterminate",
      DiscriminatorRule.Regex("a"),
      DiscriminatorRule.Contains("a"),
      Indeterminate
    ),
    ClassificationCase(
      "regex_empty_contains_indeterminate",
      DiscriminatorRule.Regex("a"),
      DiscriminatorRule.Contains(""),
      Indeterminate
    ),
    ClassificationCase(
      "prefix_field_equals_disjoint",
      DiscriminatorRule.Prefix("a"),
      fieldEquals("kind", Some("a")),
      Disjoint
    ),
    ClassificationCase(
      "prefix_field_absent_disjoint",
      DiscriminatorRule.Prefix("a"),
      DiscriminatorRule.FieldAbsent("kind"),
      Disjoint
    ),
    ClassificationCase(
      "suffix_field_equals_disjoint",
      DiscriminatorRule.Suffix("a"),
      fieldEquals("kind", Some("a")),
      Disjoint
    ),
    ClassificationCase(
      "suffix_field_absent_disjoint",
      DiscriminatorRule.Suffix("a"),
      DiscriminatorRule.FieldAbsent("kind"),
      Disjoint
    ),
    ClassificationCase(
      "contains_field_equals_disjoint",
      DiscriminatorRule.Contains("a"),
      fieldEquals("kind", Some("a")),
      Disjoint
    ),
    ClassificationCase(
      "contains_field_absent_disjoint",
      DiscriminatorRule.Contains("a"),
      DiscriminatorRule.FieldAbsent("kind"),
      Disjoint
    ),
    ClassificationCase(
      "regex_field_equals_disjoint",
      DiscriminatorRule.Regex("a"),
      fieldEquals("kind", Some("a")),
      Disjoint
    ),
    ClassificationCase(
      "regex_field_absent_disjoint",
      DiscriminatorRule.Regex("a"),
      DiscriminatorRule.FieldAbsent("kind"),
      Disjoint
    ),
    ClassificationCase(
      "field_equals_same_field_different_literals_disjoint",
      fieldEquals("kind", Some("a")),
      fieldEquals("kind", Some("b")),
      Disjoint
    ),
    ClassificationCase(
      "field_equals_same_field_same_literal_reject",
      fieldEquals("kind", Some("a")),
      fieldEquals("kind", Some("a")),
      Reject
    ),
    ClassificationCase(
      "field_equals_same_field_one_literal_absent_reject",
      fieldEquals("kind", None),
      fieldEquals("kind", Some("a")),
      Reject
    ),
    ClassificationCase(
      "field_equals_same_field_both_literals_absent_reject",
      fieldEquals("kind", None),
      fieldEquals("kind", None),
      Reject
    ),
    ClassificationCase(
      "field_equals_different_fields_reject",
      fieldEquals("left", Some("a")),
      fieldEquals("right", Some("b")),
      Reject
    ),
    ClassificationCase(
      "field_absent_same_field_reject",
      DiscriminatorRule.FieldAbsent("kind"),
      DiscriminatorRule.FieldAbsent("kind"),
      Reject
    ),
    ClassificationCase(
      "field_absent_different_fields_reject",
      DiscriminatorRule.FieldAbsent("left"),
      DiscriminatorRule.FieldAbsent("right"),
      Reject
    ),
    ClassificationCase(
      "field_equals_field_absent_same_field_disjoint",
      fieldEquals("kind", Some("a")),
      DiscriminatorRule.FieldAbsent("kind"),
      Disjoint
    ),
    ClassificationCase(
      "field_equals_field_absent_different_fields_reject",
      fieldEquals("left", Some("a")),
      DiscriminatorRule.FieldAbsent("right"),
      Reject
    )
  )
  private def classificationMatches(
    actual: WellFormedness.DiscriminatorPairClassification,
    expected: ExpectedClassification
  ) =
    (actual, expected) match {
      case (WellFormedness.DiscriminatorPairClassification.Reject(_), Reject)            => true
      case (WellFormedness.DiscriminatorPairClassification.Disjoint, Disjoint)           => true
      case (WellFormedness.DiscriminatorPairClassification.Indeterminate, Indeterminate) => true
      case _                                                                             => false
    }
  override def spec = suite("WellFormednessSpec")(
    suite("discriminator_pair_classification_matches_portable_matrix")(
      classificationCases.map(c =>
        test(c.name) {
          val forward = WellFormedness.classifyDiscriminatorPair(c.left, c.right)
          val reverse = WellFormedness.classifyDiscriminatorPair(c.right, c.left)
          assertTrue(classificationMatches(forward, c.expected), classificationMatches(reverse, c.expected))
        }
      )
    ),
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
    test("text body is not raw-JSON string-shaped") {
      val text = SchemaType(TextType(TextRestrictions()))
      val u    = SchemaType(UnionType(List(UnionBranch("t", text, DiscriminatorRule.Prefix("x")))))
      assertTrue(WellFormedness.validateGraph(graph(u)).left.exists(_.contains(UnionStringRuleOnNonStringBody("t"))))
    },
    test("text field does not support a field-equals literal") {
      val body = t.record(List(t.field("kind", SchemaType(TextType(TextRestrictions())))))
      val u    = SchemaType(UnionType(List(UnionBranch("t", body, fieldEquals("kind", Some("x"))))))
      assertTrue(
        WellFormedness
          .validateGraph(graph(u))
          .left
          .exists(_.contains(UnionFieldEqualsLiteralOnNonStringField("t", "kind")))
      )
    },
    test("public validator rejects overlap and accepts disjoint and indeterminate pairs") {
      def union(a: DiscriminatorRule, b: DiscriminatorRule) =
        graph(SchemaType(UnionType(List(UnionBranch("a", t.string, a), UnionBranch("b", t.string, b)))))
      val rejected      = WellFormedness.validateGraph(union(DiscriminatorRule.Prefix("a"), DiscriminatorRule.Suffix("b")))
      val disjoint      = WellFormedness.validateGraph(union(DiscriminatorRule.Prefix("a"), DiscriminatorRule.Prefix("b")))
      val indeterminate =
        WellFormedness.validateGraph(union(DiscriminatorRule.Regex("a.*"), DiscriminatorRule.Prefix("a")))
      assertTrue(
        rejected.left.exists(_.exists(_.isInstanceOf[UnionAmbiguousDiscriminators])),
        disjoint.isRight,
        indeterminate.isRight
      )
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
