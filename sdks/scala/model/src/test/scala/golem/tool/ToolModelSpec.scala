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
import golem.schema.SchemaTypeBody._
import golem.schema.SchemaValue._
import golem.schema.validation.ValueValidation
import zio.test._

import scala.collection.immutable.ListMap

object ToolModelSpec extends ZIOSpecDefault {

  private def doc(summary: String): Doc = Doc(summary, "", Nil)
  private def strGraph(): SchemaGraph   = SchemaGraph(ListMap.empty, t.string)
  private def u32Graph(): SchemaGraph   = SchemaGraph(ListMap.empty, t.u32)

  private def scalarOpt(long: String, alias: Option[String]): ExtendedOptionSpec =
    ExtendedOptionSpec(
      long,
      None,
      alias.toList,
      doc(""),
      None,
      ExtendedOptionShape.Scalar(strGraph()),
      None,
      false,
      None
    )

  private def boolFlag(long: String, alias: Option[String]): FlagSpec =
    FlagSpec(
      long,
      None,
      alias.toList,
      doc(""),
      FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = false)),
      None
    )

  private def emptyBody(): ExtendedCommandBody =
    ExtendedCommandBody(ExtendedPositionals.empty, Nil, Nil, Nil, None, None, None, Nil, None)

  private def leafToolWithBody(body: ExtendedCommandBody): ExtendedToolType =
    ExtendedToolType("0.1.0", Vector(ExtendedCommandNode("t", Nil, doc(""), ExtendedGlobals.empty, Nil, Some(body))))

  private def leafToolWithGlobals(globals: ExtendedGlobals): ExtendedToolType =
    ExtendedToolType("0.1.0", Vector(ExtendedCommandNode("t", Nil, doc(""), globals, Nil, None)))

  private def dispatcherChild(): ExtendedToolType =
    ExtendedToolType(
      "x",
      Vector(
        ExtendedCommandNode("child", Nil, doc(""), ExtendedGlobals.empty, List(1), None),
        ExtendedCommandNode("leaf", Nil, doc(""), ExtendedGlobals.empty, Nil, None)
      )
    )

  private def sampleTool(): ExtendedToolType =
    ExtendedToolType(
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
                false,
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
            ExtendedCommandBody(
              ExtendedPositionals(
                List(ExtendedPositional("input", doc("input"), None, strGraph(), None, true, false)),
                None
              ),
              List(
                ExtendedOptionSpec(
                  "config",
                  None,
                  Nil,
                  doc("config"),
                  None,
                  ExtendedOptionShape.RepeatableMap(
                    ExtendedRepeatableMapShape(
                      Repetition.Repeated,
                      SchemaGraph(ListMap.empty, t.map(t.string, t.string)),
                      DuplicateKeyPolicy.Reject
                    )
                  ),
                  None,
                  false,
                  None
                )
              ),
              List(
                FlagSpec(
                  "force",
                  None,
                  Nil,
                  doc("force"),
                  FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = false)),
                  None
                )
              ),
              Nil,
              None,
              None,
              Some(ExtendedResultSpec(strGraph(), doc("result"), List(Formatter("human", doc("human"))), "human")),
              Nil,
              None
            )
          )
        )
      )
    )

  private def toolWithInheritedGlobalAndBodyOption(
    globalLong: String,
    globalAliases: List[String],
    bodyLong: String,
    bodyAliases: List[String]
  ): ExtendedToolType =
    ExtendedToolType(
      "0.1.0",
      Vector(
        ExtendedCommandNode(
          "root",
          Nil,
          doc("root"),
          ExtendedGlobals(
            List(
              ExtendedOptionSpec(
                globalLong,
                None,
                globalAliases,
                doc("global"),
                None,
                ExtendedOptionShape.Scalar(u32Graph()),
                None,
                false,
                None
              )
            ),
            Nil
          ),
          List(1),
          None
        ),
        ExtendedCommandNode(
          "leaf",
          Nil,
          doc("leaf"),
          ExtendedGlobals.empty,
          Nil,
          Some(
            emptyBody().copy(options =
              List(
                ExtendedOptionSpec(
                  bodyLong,
                  None,
                  bodyAliases,
                  doc("body"),
                  None,
                  ExtendedOptionShape.Scalar(strGraph()),
                  None,
                  false,
                  None
                )
              )
            )
          )
        )
      )
    )

  private def graft(
    child: ExtendedToolType,
    expectedName: String,
    parentGlobals: ExtendedGlobals,
    annotations: Option[CommandAnnotations] = None,
    overrideName: Option[String] = None
  ): Either[ToolBuildError, Vector[ExtendedCommandNode]] =
    ToolComposition.graftSubtree(child, expectedName, parentGlobals, Nil, overrideName, None, None, annotations)

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolModelSpec")(
      test("builds_tool_and_orders_fields") {
        val tool  = sampleTool()
        val wire  = tool.toTool
        val names = tool.canonicalInputFields(1).map(_.name)
        assertTrue(
          wire.commands.nodes.length == 2,
          wire.commands.nodes(1).body.get.options.length == 1,
          names == List("verbose", "input", "config", "force")
        )
      },
      test("canonical_input_model_builds_record_schema_in_field_order") {
        val model = sampleTool().canonicalInputModel(1).toOption.get
        val names = model.recordSchema.root.body match { case RecordType(fields) => fields.map(_.name); case _ => Nil }
        assertTrue(
          model.fields.map(_.name) == List("verbose", "input", "config", "force"),
          names == List("verbose", "input", "config", "force")
        )
      },
      test("canonical_input_model_decodes_positional_record_by_index") {
        val decoded = sampleTool()
          .canonicalInputModel(1)
          .toOption
          .get
          .decodeRecord(RecordValue(List(U32Value(3L), StringValue("in.txt"), MapValue(Nil), BoolValue(true))))
          .toOption
          .get
        assertTrue(
          decoded.map(_.name) == List("verbose", "input", "config", "force"),
          decoded(0).value == U32Value(3L),
          decoded(1).value == StringValue("in.txt"),
          decoded(3).value == BoolValue(true)
        )
      },
      test("canonical_input_model_rejects_non_record_and_wrong_field_count") {
        val model = sampleTool().canonicalInputModel(1).toOption.get
        assertTrue(
          model.decodeRecord(StringValue("nope")) == Left(CanonicalInputDecodeError.ExpectedRecord),
          model.decodeRecord(RecordValue(Nil)) == Left(CanonicalInputDecodeError.FieldCountMismatch(4, 0))
        )
      },
      test("canonical_input_model_rejects_out_of_bounds_command_index") {
        val tool = sampleTool()
        assertTrue(
          tool.canonicalInputModel(99).left.toOption.get == ToolBuildError.CommandIndexOutOfBounds(99, 2),
          tool.canonicalInputRecordSchema(99).left.toOption.get == ToolBuildError.CommandIndexOutOfBounds(99, 2),
          tool.decodeCanonicalInputRecord(99, RecordValue(Nil)) == Left(
            CanonicalInputDecodeError.Model(ToolBuildError.CommandIndexOutOfBounds(99, 2))
          )
        )
      },
      test("canonical_input_model_validates_synthesized_record_schema") {
        val error = CanonicalInputModel
          .fromFields(List(CanonicalInputField("same", Nil, strGraph()), CanonicalInputField("same", Nil, u32Graph())))
          .left
          .toOption
          .get
        assertTrue(error.isInstanceOf[ToolBuildError.IllFormedSchema])
      },
      test("canonical_input_fields_body_alias_shadows_inherited_global") {
        assertTrue(
          toolWithInheritedGlobalAndBodyOption("verbose", Nil, "local", List("verbose"))
            .canonicalInputFields(1)
            .map(_.name) == List("local")
        )
      },
      test("canonical_input_fields_inherited_global_alias_is_shadowed") {
        assertTrue(
          toolWithInheritedGlobalAndBodyOption("global", List("v"), "v", Nil)
            .canonicalInputFields(1)
            .map(_.name) == List("v")
        )
      },
      test("help_contains_names") {
        val help = ToolHelp.renderHelp(sampleTool(), List("run")).toOption.get
        assertTrue(help.contains("run"), help.contains("--config"))
      },
      test("graft_accepts_root_with_body") {
        assertTrue(graft(leafToolWithBody(emptyBody()), "t", ExtendedGlobals.empty).toOption.get(0).body.isDefined)
      },
      test("graft_deprojects_child_body_against_parent_globals") {
        val parentGlobals = ExtendedGlobals(List(scalarOpt("verbose", None)), Nil)
        val childBody     = emptyBody().copy(options = List(scalarOpt("verbose", None).copy(doc = doc("local verbose"))))
        val g             = graft(leafToolWithBody(childBody), "t", parentGlobals).toOption.get
        assertTrue(!g(0).body.get.options.exists(_.long == "verbose"), g(0).globals.options.exists(_.long == "verbose"))
      },
      test("graft_rejects_incompatible_child_body_vs_parent_global") {
        val err = graft(
          leafToolWithBody(emptyBody().copy(options = List(scalarOpt("verbose", None)))),
          "t",
          ExtendedGlobals(Nil, List(boolFlag("verbose", None)))
        ).left.toOption.get
        assertTrue(err match {
          case ToolBuildError.InheritedGlobalConflict("verbose", _, "t") => true; case _ => false
        })
      },
      test("graft_deprojects_child_root_globals_against_parent_globals") {
        val g = graft(
          leafToolWithGlobals(ExtendedGlobals(Nil, List(boolFlag("verbose", None)))),
          "t",
          ExtendedGlobals(Nil, List(boolFlag("verbose", None)))
        ).toOption.get
        assertTrue(g(0).globals.flags.count(_.long == "verbose") == 1)
      },
      test("graft_rejects_incompatible_child_root_global_vs_parent_global") {
        val err = graft(
          leafToolWithGlobals(ExtendedGlobals(List(scalarOpt("verbose", None)), Nil)),
          "t",
          ExtendedGlobals(Nil, List(boolFlag("verbose", None)))
        ).left.toOption.get
        assertTrue(err match {
          case ToolBuildError.InheritedGlobalConflict("verbose", _, "t") => true; case _ => false
        })
      },
      test("graft_preserves_local_indices") {
        val g                   = graft(dispatcherChild(), "child", ExtendedGlobals.empty).toOption.get
        val parent              = Vector(ExtendedCommandNode("root", Nil, doc(""), ExtendedGlobals.empty, Nil, None))
        val (newParent, offset) = ToolComposition.appendGraftedSubtree(parent, g)
        assertTrue(
          g.length == 2,
          g(0).body.isEmpty,
          g(0).subcommands == List(1),
          offset == 1,
          newParent(1).subcommands == List(2),
          newParent.length == 3
        )
      },
      test("graft_enforces_name_rule_and_rejects_annotations") {
        val mismatch = graft(dispatcherChild(), "remote", ExtendedGlobals.empty).left.toOption.get
        val ok       = graft(dispatcherChild(), "remote", ExtendedGlobals.empty, None, Some("remote")).toOption.get
        val ann      = graft(
          dispatcherChild(),
          "child",
          ExtendedGlobals.empty,
          Some(CommandAnnotations(readOnly = true, destructive = false, idempotent = false, openWorld = false))
        ).left.toOption.get
        assertTrue(
          mismatch.isInstanceOf[ToolBuildError.SubtreeRootNameMismatch],
          ok(0).name == "remote",
          ann.isInstanceOf[ToolBuildError.SubtreeAnnotationsUnsupported]
        )
      },
      test("cycle_detection_and_refinements_work") {
        val ctx  = new ToolBuildCtx()
        val _    = ctx.pushDescriptor("a")
        val text = ToolRefinement.refineText(t.string, Some("x+"), Some(1), Some(3)).toOption.get
        val url  = ToolRefinement.refineUrl(SchemaType(UrlType(UrlRestrictions.empty)), Some(List("https"))).toOption.get
        val num  = ToolRefinement.refineNumeric(t.u32, Some(NumericBound.Unsigned(1L)), None, Some("ms")).toOption.get
        assertTrue(
          ctx.pushDescriptor("a").left.toOption.get.isInstanceOf[ToolBuildError.SubtreeCycle],
          text.body.isInstanceOf[TextType],
          url.body.isInstanceOf[UrlType],
          num.body.asInstanceOf[U32Type].restrictions.get.unit == Some("ms"),
          ToolRefinement.refineNumeric(t.string, None, None, Some("ms")).left.toOption.exists {
            case ToolBuildError.RefinementTypeMismatch("numeric", _) => true; case _ => false
          },
          ToolRefinement.refinePath(t.string, None, None, None).left.toOption.exists {
            case ToolBuildError.RefinementTypeMismatch("path", _) => true; case _ => false
          },
          ToolRefinement.refineUrl(t.string, Some(List("https"))).left.toOption.exists {
            case ToolBuildError.RefinementTypeMismatch("url", _) => true; case _ => false
          },
          ToolRefinement.refineText(t.u32, Some("x+"), None, None).left.toOption.exists {
            case ToolBuildError.RefinementTypeMismatch("text", _) => true; case _ => false
          }
        )
      },
      test("refine_numeric_overlays_existing_restrictions") {
        val base =
          ToolRefinement.refineNumeric(t.u32, Some(NumericBound.Unsigned(10L)), None, Some("items")).toOption.get
        val refined = ToolRefinement.refineNumeric(base, None, Some(NumericBound.Unsigned(20L)), None).toOption.get
        val r       = refined.body.asInstanceOf[U32Type].restrictions.get
        assertTrue(
          r.min == Some(NumericBound.Unsigned(10L)),
          r.max == Some(NumericBound.Unsigned(20L)),
          r.unit == Some("items")
        )
      },
      test("refine_numeric_preserves_unspecified_existing_restrictions") {
        val base = SchemaType(
          U32Type(
            NumericRestrictions(
              Some(NumericBound.Unsigned(10L)),
              Some(NumericBound.Unsigned(100L)),
              Some("items")
            ).normalize
          )
        )
        val refined = ToolRefinement.refineNumeric(base, None, Some(NumericBound.Unsigned(200L)), None).toOption.get
        val r       = refined.body.asInstanceOf[U32Type].restrictions.get
        assertTrue(
          r.min == Some(NumericBound.Unsigned(10L)),
          r.max == Some(NumericBound.Unsigned(200L)),
          r.unit == Some("items")
        )
      },
      test("numeric_value_validation_rejects_malformed_restrictions") {
        val ty = SchemaType(
          U32Type(Some(NumericRestrictions(Some(NumericBound.Unsigned(10L)), Some(NumericBound.Unsigned(1L)), None)))
        )
        val graph = SchemaGraph(ListMap.empty, ty)
        assertTrue(ValueValidation.validateValue(graph, ty, U32Value(5L)).isLeft)
      }
    )
}
