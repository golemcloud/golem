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

object ToolValidationSpec extends ZIOSpecDefault {
  private def doc(summary: String): Doc            = Doc(summary, "")
  private def graph(root: SchemaType): SchemaGraph = SchemaGraph(ListMap.empty, root)
  private def strGraph(): SchemaGraph              = graph(t.string)
  private def u32Graph(): SchemaGraph              = graph(t.u32)
  private def emptyBody(): ExtendedCommandBody     =
    ExtendedCommandBody(ExtendedPositionals.empty, Nil, Nil, Nil, None, None, None, Nil, None)
  private def leafToolWithBody(body: ExtendedCommandBody): ExtendedToolType =
    ExtendedToolType("0.1.0", Vector(ExtendedCommandNode("t", Nil, doc(""), ExtendedGlobals.empty, Nil, Some(body))))
  private def mapConfigOption(constraints: List[ExtendedConstraint]): ExtendedCommandBody =
    emptyBody().copy(
      options = List(
        ExtendedOptionSpec(
          "config",
          None,
          Nil,
          doc(""),
          None,
          ExtendedOptionShape.RepeatableMap(
            ExtendedRepeatableMapShape(Repetition.Repeated, graph(t.map(t.string, t.u32)), DuplicateKeyPolicy.Reject)
          ),
          None,
          required = false,
          None
        )
      ),
      constraints = constraints
    )
  private def deferredValueIsTree(
    parentGlobals: ExtendedGlobals,
    constraints: List[ExtendedConstraint]
  ): ExtendedToolType =
    toolWithNodes(
      Vector(
        ExtendedCommandNode("root", Nil, doc(""), parentGlobals, List(1), None),
        ExtendedCommandNode(
          "leaf",
          Nil,
          doc(""),
          ExtendedGlobals.empty,
          Nil,
          Some(emptyBody().copy(constraints = constraints))
        )
      )
    )
  private def firstValueIs(body: ExtendedCommandBody): ExtendedValueIsRef = body.constraints.head match {
    case ExtendedConstraint.RequiresAll(ExtendedRef.ValueIs(v) :: Nil) => v
    case other                                                         => throw new RuntimeException("expected value-is, got " + other)
  }
  private def bareNode(name: String, subcommands: List[Int]): ExtendedCommandNode =
    ExtendedCommandNode(name, Nil, doc(""), ExtendedGlobals.empty, subcommands, None)
  private def toolWithNodes(nodes: Vector[ExtendedCommandNode]): ExtendedToolType = ExtendedToolType("0.1.0", nodes)
  private def scalarOpt(long: String, short: Option[Char]): ExtendedOptionSpec    =
    ExtendedOptionSpec(
      long,
      short,
      Nil,
      doc(""),
      None,
      ExtendedOptionShape.Scalar(strGraph()),
      None,
      required = false,
      None
    )
  private def boolFlag(long: String, short: Option[Char]): FlagSpec =
    FlagSpec(long, short, Nil, doc(""), FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = false)), None)
  private def variantGraph(): SchemaGraph                                                 = graph(t.variant(List(t.variantCase("case"))))
  private def refGraph(id: String): SchemaGraph                                           = graph(t.ref(id))
  private def validate(tool: ExtendedToolType): Either[ToolBuildError, Unit]              = ToolValidation.validateTool(tool)
  private def normalize(tool: ExtendedToolType): Either[ToolBuildError, ExtendedToolType] =
    ToolComposition.normalizeInheritedGlobals(tool)
  private def shapesMatch(a: SchemaType, b: SchemaType): Boolean                    = ToolGraphs.schemaShapesMatch(graph(a), graph(b))
  private def quantity(baseUnit: String, allowedSuffixes: List[String]): SchemaType = SchemaType(
    SchemaTypeBody.QuantityType(QuantitySpec(baseUnit, allowedSuffixes))
  )
  private def sampleTool(): ExtendedToolType = ExtendedToolType(
    "0.1.0",
    Vector(
      ExtendedCommandNode(
        "root",
        Nil,
        doc("root"),
        ExtendedGlobals(
          List(
            ExtendedOptionSpec(
              "verbose",
              None,
              Nil,
              doc("global"),
              None,
              ExtendedOptionShape.Scalar(u32Graph()),
              None,
              required = false,
              None
            )
          ),
          Nil
        ),
        List(1),
        None
      ),
      ExtendedCommandNode(
        "run",
        List("r"),
        doc("run"),
        ExtendedGlobals.empty,
        Nil,
        Some(
          emptyBody().copy(
            positionals = ExtendedPositionals(
              List(
                ExtendedPositional("input", doc("input"), None, strGraph(), None, required = true, acceptsStdio = false)
              ),
              None
            ),
            options = List(
              ExtendedOptionSpec(
                "config",
                None,
                Nil,
                doc("config"),
                None,
                ExtendedOptionShape.RepeatableMap(
                  ExtendedRepeatableMapShape(
                    Repetition.Repeated,
                    graph(t.map(t.string, t.string)),
                    DuplicateKeyPolicy.Reject
                  )
                ),
                None,
                required = false,
                None
              )
            ),
            flags = List(boolFlag("force", None)),
            result =
              Some(ExtendedResultSpec(strGraph(), doc("result"), List(Formatter("human", doc("human"))), "human"))
          )
        )
      )
    )
  )

  def spec: Spec[Any, Any] = suite("ToolValidationSpec")(
    test("default_type_mismatch_is_rejected") {
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          List(
            ExtendedPositional(
              "count",
              doc(""),
              None,
              u32Graph(),
              Some(SchemaValue.StringValue("not-a-number")),
              required = false,
              acceptsStdio = false
            )
          ),
          None
        )
      )
      assertTrue(
        leafToolWithBody(body).tryToTool.left.toOption.exists(_.isInstanceOf[ToolBuildError.DefaultTypeMismatch])
      )
    },
    test("value_is_resolves_to_map_value_type") {
      val ok = leafToolWithBody(
        mapConfigOption(
          List(
            ExtendedConstraint.RequiresAll(
              List(
                ExtendedRef.ValueIs(
                  ExtendedValueIsRef("config", ExtendedValueIsLiteral.Resolved(SchemaValue.U32Value(1)))
                )
              )
            )
          )
        )
      )
      val bad = leafToolWithBody(
        mapConfigOption(
          List(
            ExtendedConstraint.RequiresAll(
              List(
                ExtendedRef.ValueIs(
                  ExtendedValueIsRef("config", ExtendedValueIsLiteral.Resolved(SchemaValue.StringValue("x")))
                )
              )
            )
          )
        )
      )
      assertTrue(
        ok.tryToTool.isRight,
        bad.tryToTool.left.toOption.exists(_.isInstanceOf[ToolBuildError.ValueIsTypeMismatch])
      )
    },
    test("unresolved_constraint_ref_is_rejected") {
      val body = mapConfigOption(List(ExtendedConstraint.RequiresAll(List(ExtendedRef.Present("missing")))))
      assertTrue(
        leafToolWithBody(body).tryToTool.left.toOption.exists(_.isInstanceOf[ToolBuildError.UnresolvedConstraintRef])
      )
    },
    test("deferred_value_is_resolves_against_ancestor_global") {
      val tool = deferredValueIsTree(
        ExtendedGlobals(List(scalarOpt("format", None)), Nil),
        List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("format", ExtendedValueIsLiteral.Deferred(ToolLiteral.StrLiteral("json")))
              )
            )
          )
        )
      )
      val normalized = normalize(tool).toOption.get
      val resolved   = firstValueIs(normalized.commands(1).body.get)
      assertTrue(
        resolved.value == ExtendedValueIsLiteral.Resolved(SchemaValue.StringValue("json")),
        normalized.tryToTool.isRight
      )
    },
    test("deferred_value_is_unknown_name_is_rejected") {
      val tool = deferredValueIsTree(
        ExtendedGlobals.empty,
        List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("missing", ExtendedValueIsLiteral.Deferred(ToolLiteral.StrLiteral("json")))
              )
            )
          )
        )
      )
      val normalized = normalize(tool).toOption.get
      assertTrue(normalized.tryToTool.left.toOption.contains(ToolBuildError.UnresolvedConstraintRef("missing")))
    },
    test("deferred_value_is_incompatible_literal_is_rejected") {
      val tool = deferredValueIsTree(
        ExtendedGlobals(List(scalarOpt("format", None)), Nil),
        List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("format", ExtendedValueIsLiteral.Deferred(ToolLiteral.BoolLiteral(true)))
              )
            )
          )
        )
      )
      assertTrue(normalize(tool).left.toOption.contains(ToolBuildError.ValueIsTypeMismatch("format")))
    },
    test("deferred_value_is_against_ancestor_flag_is_rejected") {
      val tool = deferredValueIsTree(
        ExtendedGlobals(Nil, List(boolFlag("force", None))),
        List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("force", ExtendedValueIsLiteral.Deferred(ToolLiteral.BoolLiteral(true)))
              )
            )
          )
        )
      )
      assertTrue(normalize(tool).left.toOption.contains(ToolBuildError.ValueIsTypeMismatch("force")))
    },
    test("deferred_value_is_unresolved_at_validation_is_rejected") {
      val body = emptyBody().copy(
        options = List(scalarOpt("format", None)),
        constraints = List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("format", ExtendedValueIsLiteral.Deferred(ToolLiteral.StrLiteral("json")))
              )
            )
          )
        )
      )
      assertTrue(
        validate(leafToolWithBody(body)).left.toOption.contains(ToolBuildError.UnresolvedValueIsLiteral("format"))
      )
    },
    test("empty_tree_is_rejected") {
      assertTrue(validate(toolWithNodes(Vector.empty)).left.toOption.contains(ToolBuildError.EmptyCommandTree))
    },
    test("out_of_bounds_subcommand_is_rejected") {
      assertTrue(
        validate(toolWithNodes(Vector(bareNode("root", List(5))))).left.toOption
          .exists(_.isInstanceOf[ToolBuildError.CommandIndexOutOfBounds]),
        validate(toolWithNodes(Vector(bareNode("root", List(-1))))).left.toOption
          .exists(_.isInstanceOf[ToolBuildError.CommandIndexOutOfBounds])
      )
    },
    test("canonical_input_model_rejects_invalid_subcommand_index_in_reachable_tree") {
      val tool = toolWithNodes(Vector(bareNode("root", List(1, -1)), bareNode("child", Nil)))
      assertTrue(
        validate(tool).left.toOption.contains(ToolBuildError.CommandIndexOutOfBounds(-1, 2)),
        tool.canonicalInputModel(1).left.toOption.contains(ToolBuildError.CommandIndexOutOfBounds(-1, 2)),
        tool.canonicalInputRecordSchema(1).left.toOption.contains(ToolBuildError.CommandIndexOutOfBounds(-1, 2)),
        tool
          .decodeCanonicalInputRecord(1, SchemaValue.RecordValue(Nil))
          .left
          .toOption
          .contains(CanonicalInputDecodeError.Model(ToolBuildError.CommandIndexOutOfBounds(-1, 2)))
      )
    },
    test("cyclic_tree_is_rejected_without_panicking") {
      val tool    = toolWithNodes(Vector(bareNode("root", List(1)), bareNode("child", List(0))))
      val globals = tool.effectiveGlobals(1)
      assertTrue(
        validate(tool).left.toOption.exists(_.isInstanceOf[ToolBuildError.CommandTreeCycle]),
        ToolHelp.renderHelp(tool, Nil).isRight,
        globals.isEmpty || globals.nonEmpty
      )
    },
    test("canonical_input_model_rejects_cycle_on_target_path") {
      val tool = toolWithNodes(Vector(bareNode("root", List(1)), bareNode("child", List(0))))
      assertTrue(
        tool.canonicalInputModel(1).left.toOption.exists(_.isInstanceOf[ToolBuildError.CommandTreeCycle]),
        tool.decodeCanonicalInputRecord(1, SchemaValue.RecordValue(Nil)).left.toOption.exists {
          case CanonicalInputDecodeError.Model(_: ToolBuildError.CommandTreeCycle) => true; case _ => false
        }
      )
    },
    test("shared_subcommand_is_rejected") {
      val tool = toolWithNodes(
        Vector(bareNode("root", List(1, 2)), bareNode("a", List(3)), bareNode("b", List(3)), bareNode("leaf", Nil))
      );
      assertTrue(validate(tool).left.toOption.contains(ToolBuildError.DuplicateCommandParent(3)))
    },
    test("canonical_input_model_rejects_duplicate_parent_command_path") {
      val a    = bareNode("a", List(3)).copy(globals = ExtendedGlobals(List(scalarOpt("from-a", None)), Nil));
      val b    = bareNode("b", List(3)).copy(globals = ExtendedGlobals(List(scalarOpt("from-b", None)), Nil));
      val tool = toolWithNodes(Vector(bareNode("root", List(1, 2)), a, b, bareNode("leaf", Nil)));
      assertTrue(
        validate(tool).left.toOption.contains(ToolBuildError.DuplicateCommandParent(3)),
        tool.canonicalInputModel(3).left.toOption.contains(ToolBuildError.DuplicateCommandParent(3))
      )
    },
    test("unreachable_node_is_rejected") {
      assertTrue(
        validate(toolWithNodes(Vector(bareNode("root", Nil), bareNode("orphan", Nil)))).left.toOption
          .contains(ToolBuildError.UnreachableCommandNode(1))
      )
    },
    test("invalid_identifier_is_rejected") {
      assertTrue(
        validate(toolWithNodes(Vector(bareNode("Root", Nil)))).left.toOption
          .exists(_.isInstanceOf[ToolBuildError.InvalidIdentifier])
      )
    },
    test("body_name_colliding_with_inherited_global_is_rejected") {
      val root  = bareNode("root", List(1)).copy(globals = ExtendedGlobals(List(scalarOpt("shared", None)), Nil));
      val child = bareNode("child", Nil).copy(body = Some(emptyBody().copy(options = List(scalarOpt("shared", None)))));
      assertTrue(
        validate(toolWithNodes(Vector(root, child))).left.toOption.exists(_.isInstanceOf[ToolBuildError.DuplicateName])
      )
    },
    test("duplicate_short_form_is_rejected") {
      val body = emptyBody().copy(options = List(scalarOpt("alpha", Some('a')), scalarOpt("beta", Some('a'))));
      assertTrue(validate(leafToolWithBody(body)).left.toOption.contains(ToolBuildError.DuplicateShort('a')))
    },
    test("verbatim_tail_without_separator_is_rejected") {
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          Nil,
          Some(
            ExtendedTailPositional(
              "args",
              doc(""),
              None,
              strGraph(),
              0,
              None,
              None,
              verbatim = true,
              acceptsStdio = false
            )
          )
        )
      );
      assertTrue(
        validate(leafToolWithBody(body)).left.toOption.exists(_.isInstanceOf[ToolBuildError.VerbatimWithoutSeparator])
      )
    },
    test("variant_in_input_position_is_rejected") {
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          List(
            ExtendedPositional("choice", doc(""), None, variantGraph(), None, required = true, acceptsStdio = false)
          ),
          None
        )
      );
      assertTrue(
        validate(leafToolWithBody(body)).left.toOption.exists(_.isInstanceOf[ToolBuildError.VariantInInputPosition])
      )
    },
    test("value_is_against_flag_is_rejected") {
      val body = emptyBody().copy(
        flags = List(boolFlag("force", None)),
        constraints = List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("force", ExtendedValueIsLiteral.Resolved(SchemaValue.BoolValue(true)))
              )
            )
          )
        )
      );
      assertTrue(
        validate(leafToolWithBody(body)).left.toOption.exists(_.isInstanceOf[ToolBuildError.ValueIsTypeMismatch])
      )
    },
    test("repeatable_map_with_non_map_type_is_rejected") {
      val body = emptyBody().copy(options =
        List(
          ExtendedOptionSpec(
            "config",
            None,
            Nil,
            doc(""),
            None,
            ExtendedOptionShape.RepeatableMap(
              ExtendedRepeatableMapShape(Repetition.Repeated, strGraph(), DuplicateKeyPolicy.Reject)
            ),
            None,
            required = false,
            None
          )
        )
      );
      assertTrue(
        validate(leafToolWithBody(body)).left.toOption.exists(_.isInstanceOf[ToolBuildError.RepeatableMapTypeNotMap])
      )
    },
    test("deferred_value_is_does_not_mask_repeatable_map_type_error") {
      val body = emptyBody().copy(
        options = List(
          ExtendedOptionSpec(
            "config",
            None,
            Nil,
            doc(""),
            None,
            ExtendedOptionShape.RepeatableMap(
              ExtendedRepeatableMapShape(Repetition.Repeated, strGraph(), DuplicateKeyPolicy.Reject)
            ),
            None,
            required = false,
            None
          )
        ),
        constraints = List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("config", ExtendedValueIsLiteral.Deferred(ToolLiteral.IntLiteral(BigInt(1))))
              )
            )
          )
        )
      );
      val normalized = normalize(leafToolWithBody(body)).toOption.get;
      assertTrue(validate(normalized).left.toOption.contains(ToolBuildError.RepeatableMapTypeNotMap("config")))
    },
    test("dangling_type_ref_in_positional_is_rejected") {
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          List(
            ExtendedPositional(
              "thing",
              doc(""),
              None,
              refGraph("missing-type"),
              None,
              required = true,
              acceptsStdio = false
            )
          ),
          None
        )
      );
      assertTrue(validate(leafToolWithBody(body)).left.toOption.exists {
        case ToolBuildError.UnresolvedTypeRef(_, "missing-type") => true; case _ => false
      })
    },
    test("dangling_type_ref_in_option_is_rejected") {
      val body = emptyBody()
        .copy(options = List(scalarOpt("name", None).copy(shape = ExtendedOptionShape.Scalar(refGraph("nope")))));
      assertTrue(
        validate(leafToolWithBody(body)).left.toOption
          .contains(ToolBuildError.UnresolvedTypeRef("option --name", "nope"))
      )
    },
    test("deferred_value_is_does_not_mask_dangling_option_type_ref") {
      val body = emptyBody().copy(
        options = List(scalarOpt("name", None).copy(shape = ExtendedOptionShape.Scalar(refGraph("nope")))),
        constraints = List(
          ExtendedConstraint.RequiresAll(
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("name", ExtendedValueIsLiteral.Deferred(ToolLiteral.StrLiteral("x")))
              )
            )
          )
        )
      );
      val normalized = normalize(leafToolWithBody(body)).toOption.get;
      assertTrue(validate(normalized).left.toOption.contains(ToolBuildError.UnresolvedTypeRef("option --name", "nope")))
    },
    test("dangling_type_ref_in_definition_body_is_rejected") {
      val g    = SchemaGraph(ListMap("rec" -> SchemaTypeDef(t.list(t.ref("gone")), Some("rec"))), t.ref("rec"));
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          List(ExtendedPositional("thing", doc(""), None, g, None, required = true, acceptsStdio = false)),
          None
        )
      );
      assertTrue(validate(leafToolWithBody(body)).left.toOption.exists {
        case ToolBuildError.UnresolvedTypeRef(_, "gone") => true; case _ => false
      })
    },
    test("ill_formed_numeric_restriction_is_rejected") {
      val bad = graph(
        SchemaType(
          SchemaTypeBody.U32Type(
            Some(NumericRestrictions(Some(NumericBound.Unsigned(10)), Some(NumericBound.Unsigned(1))))
          )
        )
      );
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          List(ExtendedPositional("count", doc(""), None, bad, None, required = true, acceptsStdio = false)),
          None
        )
      );
      assertTrue(validate(leafToolWithBody(body)).left.toOption.exists {
        case ToolBuildError.IllFormedSchema("positional count", _) => true; case _ => false
      })
    },
    test("resolvable_type_ref_is_accepted") {
      val g    = SchemaGraph(ListMap("rec" -> SchemaTypeDef(t.string, Some("rec"))), t.ref("rec"));
      val body = emptyBody().copy(positionals =
        ExtendedPositionals(
          List(ExtendedPositional("thing", doc(""), None, g, None, required = true, acceptsStdio = false)),
          None
        )
      );
      assertTrue(validate(leafToolWithBody(body)).isRight)
    },
    test("argument_help_finds_global_and_body_args") {
      val tool = sampleTool(); val global = ToolHelp.renderArgumentHelp(tool, List("run"), "verbose").toOption.get;
      val body = ToolHelp.renderArgumentHelp(tool, List("run"), "config").toOption.get;
      assertTrue(
        global.contains("--verbose"),
        global.contains("global"),
        body.contains("--config"),
        ToolHelp
          .renderArgumentHelp(tool, List("run"), "nope")
          .left
          .toOption
          .exists(_.isInstanceOf[ToolBuildError.CommandNotFound])
      )
    },
    test("rich_leaf_quantity_identity_is_compared_but_bounds_ignored") {
      val lo = quantity("m", Nil);
      val hi = SchemaType(SchemaTypeBody.QuantityType(QuantitySpec("m", Nil, Some(QuantityValue(0, 0, "m")), None)));
      assertTrue(
        !shapesMatch(quantity("m", Nil), quantity("s", Nil)),
        !shapesMatch(quantity("m", List("m", "km")), quantity("m", List("m"))),
        shapesMatch(quantity("m", List("m", "km")), quantity("m", List("km", "m"))),
        shapesMatch(lo, hi)
      )
    },
    test("rich_leaf_secret_and_quota_identity_is_compared") {
      assertTrue(
        shapesMatch(
          SchemaType(SchemaTypeBody.SecretType(SecretSpec(t.string, Some("api-key")))),
          SchemaType(SchemaTypeBody.SecretType(SecretSpec(t.string, Some("api-key"))))
        ),
        !shapesMatch(
          SchemaType(SchemaTypeBody.SecretType(SecretSpec(t.string, Some("api-key")))),
          SchemaType(SchemaTypeBody.SecretType(SecretSpec(t.string, Some("oauth-token"))))
        ),
        !shapesMatch(
          SchemaType(SchemaTypeBody.QuotaTokenType(QuotaTokenSpec(Some("tokens")))),
          SchemaType(SchemaTypeBody.QuotaTokenType(QuotaTokenSpec(Some("requests"))))
        )
      )
    },
    test("rich_leaf_secret_inner_identity_is_compared") {
      assertTrue(
        !shapesMatch(
          SchemaType(SchemaTypeBody.SecretType(SecretSpec(t.string, Some("api-key")))),
          SchemaType(SchemaTypeBody.SecretType(SecretSpec(t.u64, Some("api-key"))))
        )
      )
    },
    test("rich_leaf_text_and_binary_identity_is_compared_but_other_bounds_ignored") {
      def text(languages: Option[List[String]], regex: Option[String]) =
        SchemaType(SchemaTypeBody.TextType(TextRestrictions(languages = languages, regex = regex)));
      def binary(mime: Option[List[String]], maxBytes: Option[Int]) =
        SchemaType(SchemaTypeBody.BinaryType(BinaryRestrictions(mimeTypes = mime, maxBytes = maxBytes)));
      assertTrue(
        !shapesMatch(text(Some(List("en")), None), text(Some(List("de")), None)),
        !shapesMatch(text(None, None), text(Some(List("en")), None)),
        shapesMatch(text(Some(List("en")), Some("a+")), text(Some(List("en")), Some("b+"))),
        !shapesMatch(binary(Some(List("image/png")), None), binary(Some(List("image/jpeg")), None)),
        shapesMatch(binary(Some(List("image/png")), Some(10)), binary(Some(List("image/png")), Some(20)))
      )
    },
    test("rich_leaf_path_direction_and_kind_are_refinable_and_ignored") {
      def path(direction: PathDirection, kind: PathKind) =
        SchemaType(SchemaTypeBody.PathType(PathSpec(direction, kind)));
      assertTrue(shapesMatch(path(PathDirection.Input, PathKind.File), path(PathDirection.Output, PathKind.Directory)))
    },
    test("plain_string_matches_unrestricted_text_but_not_language_restricted") {
      val unrestricted       = SchemaType(SchemaTypeBody.TextType(TextRestrictions(regex = Some("^x$"))));
      val languageRestricted = SchemaType(SchemaTypeBody.TextType(TextRestrictions(languages = Some(List("en")))));
      assertTrue(
        shapesMatch(t.string, unrestricted),
        shapesMatch(unrestricted, t.string),
        !shapesMatch(t.string, languageRestricted)
      )
    }
  )
}
