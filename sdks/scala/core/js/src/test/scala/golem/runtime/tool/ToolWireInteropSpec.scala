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

package golem.runtime.tool

import golem.host.ToolWireInterop
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.schema.wire.SchemaWire
import golem.tool.wire.{WitTool, WitToolError}
import zio.test._

import scala.scalajs.js

/**
 * Verifies that the `Wit* <-> Js*` tool mapping is lossless (roundtrips) and
 * that the produced JS objects match `golem_tool_0_1_0_common.d.ts` exactly
 * (tag strings, string enums, camelCase / `default_` field names).
 */
object ToolWireInteropSpec extends ZIOSpecDefault {
  import ToolTestFixtures._

  private lazy val richWit: WitTool =
    richTool("interop-rich").tryToTool.fold(e => throw new RuntimeException(e.toString), identity)

  private def typed(s: String) =
    SchemaWire.typedSchemaValueToWit(TypedSchemaValue(strGraph, SchemaValue.StringValue(s)))

  private def dyn(a: js.Any): js.Dynamic = a.asInstanceOf[js.Dynamic]

  /** The rich tool with the root global option's `short` replaced. */
  private def withGlobalOptionShort(short: Option[Char]): WitTool = {
    val nodes  = richWit.commands.nodes
    val root   = nodes(0)
    val option = root.globals.options.head.copy(short = short)
    richWit.copy(commands =
      golem.tool.wire.WitCommandTree(
        nodes.updated(0, root.copy(globals = root.globals.copy(options = List(option))))
      )
    )
  }

  /**
   * Encodes the rich tool and overwrites the root global option's `short` on
   * the raw JS object.
   */
  private def encodedWithRawShort(shortJs: String): js.Any = {
    val j    = ToolWireInterop.toolToJs(richWit)
    val root = dyn(dyn(j).commands.nodes.asInstanceOf[js.Array[js.Any]](0))
    dyn(root.globals.options.asInstanceOf[js.Array[js.Any]](0)).updateDynamic("short")(shortJs)
    j
  }

  private def failureOf[A](thunk: => A): Option[Throwable] =
    try { val _ = thunk; None }
    catch { case t: Throwable => Some(t) }

  def spec: Spec[Any, Any] = suite("ToolWireInteropSpec")(
    test("rich_tool_roundtrips_through_js") {
      val roundtripped = ToolWireInterop.toolFromJs(ToolWireInterop.toolToJs(richWit))
      assertTrue(roundtripped == richWit)
    },
    test("js_tool_shape_matches_dts") {
      val j    = dyn(ToolWireInterop.toolToJs(richWit))
      val root = dyn(j.commands.nodes.asInstanceOf[js.Array[js.Any]](0))
      val run  = dyn(j.commands.nodes.asInstanceOf[js.Array[js.Any]](1))
      val body = run.body

      def option(long: String): js.Dynamic =
        body.options
          .asInstanceOf[js.Array[js.Any]]
          .map(dyn)
          .find(_.long.asInstanceOf[String] == long)
          .getOrElse(throw new RuntimeException(s"option not found: $long"))

      val globalOption = dyn(root.globals.options.asInstanceOf[js.Array[js.Any]](0))
      val quiet        = dyn(root.globals.flags.asInstanceOf[js.Array[js.Any]](0))
      val verbose      = dyn(root.globals.flags.asInstanceOf[js.Array[js.Any]](1))
      val positional   = dyn(body.positionals.fixed.asInstanceOf[js.Array[js.Any]](1))
      val tail         = body.positionals.tail
      val constraints  = body.constraints.asInstanceOf[js.Array[js.Any]].map(dyn)
      val errorCase    = dyn(body.errors.asInstanceOf[js.Array[js.Any]](0))

      assertTrue(
        j.version.asInstanceOf[String] == "0.2.0",
        // option shapes and their d.ts tag strings
        dyn(option("config").shape).tag.asInstanceOf[String] == "repeatable-map",
        dyn(dyn(option("config").shape).selectDynamic("val")).duplicateKeyPolicy
          .asInstanceOf[String] == "last-wins",
        dyn(dyn(dyn(option("config").shape).selectDynamic("val")).repetition).tag
          .asInstanceOf[String] == "delimited",
        dyn(dyn(dyn(option("config").shape).selectDynamic("val")).repetition)
          .selectDynamic("val")
          .asInstanceOf[String] == ",",
        dyn(option("exclude").shape).tag.asInstanceOf[String] == "repeatable-list",
        dyn(option("output").shape).tag.asInstanceOf[String] == "scalar",
        dyn(option("opt-level").shape).tag.asInstanceOf[String] == "optional-scalar",
        // reserved-word field names use the wasm-rquickjs `default_` spelling
        !js.isUndefined(option("output").selectDynamic("default_")),
        !js.isUndefined(positional.selectDynamic("default_")),
        // `short` is a single-char string
        globalOption.short.asInstanceOf[String] == "l",
        globalOption.envVar.asInstanceOf[String] == "RICH_LEVEL",
        // positional/result type indices use the `type` field name
        !js.isUndefined(positional.selectDynamic("type")),
        !js.isUndefined(dyn(body.result).selectDynamic("type")),
        // flag shapes
        dyn(quiet.shape).tag.asInstanceOf[String] == "bool-flag",
        dyn(dyn(quiet.shape).selectDynamic("val")).negatable.asInstanceOf[Boolean] == true,
        !js.isUndefined(dyn(dyn(quiet.shape).selectDynamic("val")).selectDynamic("default_")),
        dyn(verbose.shape).tag.asInstanceOf[String] == "count-flag",
        dyn(verbose.shape).selectDynamic("val").asInstanceOf[Int] == 3,
        // tail positional
        dyn(tail).separator.asInstanceOf[String] == "--",
        dyn(tail).verbatim.asInstanceOf[Boolean] == true,
        // constraint tags in declaration order
        constraints.map(_.tag.asInstanceOf[String]).toList == List(
          "requires-all",
          "all-or-none",
          "requires-any",
          "mutex-groups",
          "implies",
          "forbids"
        ),
        dyn(constraints(4).selectDynamic("val")).lhsQuant.asInstanceOf[String] == "all",
        dyn(constraints(4).selectDynamic("val")).rhsQuant.asInstanceOf[String] == "any",
        dyn(dyn(constraints(4).selectDynamic("val")).rhs.asInstanceOf[js.Array[js.Any]](0)).tag
          .asInstanceOf[String] == "value-is",
        // error case enum + annotations
        errorCase.kind.asInstanceOf[String] == "runtime-error",
        dyn(body.annotations).destructive.asInstanceOf[Boolean] == true,
        dyn(body.annotations).openWorld.asInstanceOf[Boolean] == true
      )
    },
    test("tool_errors_roundtrip_through_js") {
      val errors: List[WitToolError] = List(
        WitToolError.InvalidToolName("nope"),
        WitToolError.InvalidCommandPath(List("a", "b")),
        WitToolError.InvalidInput("bad input"),
        WitToolError.ConstraintViolation("mutex violated"),
        WitToolError.InvalidResult("wrong type"),
        WitToolError.CustomError(typed("boom"))
      )
      val roundtripped = errors.map(e => ToolWireInterop.toolErrorFromJs(ToolWireInterop.toolErrorToJs(e)))
      assertTrue(roundtripped == errors)
    },
    test("non_ascii_bmp_char_short_roundtrips") {
      val tool = withGlobalOptionShort(Some('ä'))
      assertTrue(ToolWireInterop.toolFromJs(ToolWireInterop.toolToJs(tool)) == tool)
    },
    test("surrogate_char_short_is_rejected_on_encode") {
      val err = failureOf(ToolWireInterop.toolToJs(withGlobalOptionShort(Some('\ud800'))))
      assertTrue(err.exists(_.getMessage.contains("not a Unicode scalar value")))
    },
    test("non_bmp_char_short_is_rejected_on_decode") {
      val encoded = encodedWithRawShort("\ud83d\ude00")
      val err     = failureOf(ToolWireInterop.toolFromJs(encoded.asInstanceOf[golem.host.js.tool.JsTool]))
      assertTrue(err.exists(_.getMessage.contains("Basic Multilingual Plane")))
    },
    test("multi_code_point_char_short_is_rejected_on_decode") {
      val encoded = encodedWithRawShort("ab")
      val err     = failureOf(ToolWireInterop.toolFromJs(encoded.asInstanceOf[golem.host.js.tool.JsTool]))
      assertTrue(err.exists(_.getMessage.contains("single-code-point")))
    },
    test("tool_error_js_tags_match_dts") {
      val tags = List(
        WitToolError.InvalidToolName("x"),
        WitToolError.InvalidCommandPath(Nil),
        WitToolError.InvalidInput("x"),
        WitToolError.ConstraintViolation("x"),
        WitToolError.InvalidResult("x"),
        WitToolError.CustomError(typed("x"))
      ).map(e => dyn(ToolWireInterop.toolErrorToJs(e)).tag.asInstanceOf[String])
      assertTrue(
        tags == List(
          "invalid-tool-name",
          "invalid-command-path",
          "invalid-input",
          "constraint-violation",
          "invalid-result",
          "custom-error"
        )
      )
    }
  )
}
