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
import golem.tool.{
  CommandAnnotations,
  Doc,
  ErrorKind,
  Example,
  ToolBuildError,
  ToolMiddleware,
  ToolMiddlewareDescriptor,
  ToolMiddlewareScope
}
import golem.tool.wire.{WitCustomToolError, WitTool, WitToolError}
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
    test("tool_middleware_descriptors_roundtrip_through_js") {
      val tool        = leafTool("interop-middleware-tool").tryToTool.toOption.get
      val descriptors = List(
        ToolMiddlewareDescriptor(
          "interop-monomorphic",
          List("mono"),
          Doc("summary", "description"),
          ToolMiddlewareScope.Monomorphic(tool, Some(tool)),
          ToolMiddleware.noParametersSchema
        ),
        ToolMiddlewareDescriptor(
          "interop-universal",
          Nil,
          Doc.empty,
          ToolMiddlewareScope.Universal,
          ToolMiddleware.noParametersSchema
        )
      )
      val roundtripped =
        descriptors.map(value => ToolWireInterop.toolMiddlewareFromJs(ToolWireInterop.toolMiddlewareToJs(value)))
      assertTrue(roundtripped == descriptors)
    },
    test("tool_middleware_scope_js_shape_matches_dts") {
      val tool        = leafTool("interop-middleware-shape").tryToTool.toOption.get
      val monomorphic = dyn(
        ToolWireInterop.toolMiddlewareToJs(
          ToolMiddlewareDescriptor(
            "interop-monomorphic-shape",
            List("shape"),
            Doc.empty,
            ToolMiddlewareScope.Monomorphic(tool, None),
            ToolMiddleware.noParametersSchema
          )
        )
      )
      val universal = dyn(
        ToolWireInterop.toolMiddlewareToJs(
          ToolMiddlewareDescriptor(
            "interop-universal-shape",
            Nil,
            Doc.empty,
            ToolMiddlewareScope.Universal,
            ToolMiddleware.noParametersSchema
          )
        )
      )
      val monomorphicScope = dyn(monomorphic.scope)
      val monomorphicValue = dyn(monomorphicScope.selectDynamic("val"))
      assertTrue(
        monomorphic.name.asInstanceOf[String] == "interop-monomorphic-shape",
        monomorphic.aliases.asInstanceOf[js.Array[String]].toList == List("shape"),
        monomorphicScope.tag.asInstanceOf[String] == "monomorphic",
        !js.isUndefined(monomorphicValue.presented),
        js.isUndefined(monomorphicValue.expected),
        dyn(universal.scope).tag.asInstanceOf[String] == "universal",
        js.isUndefined(dyn(universal.scope).selectDynamic("val"))
      )
    },
    test("rich_tool_roundtrips_through_js") {
      val roundtripped = ToolWireInterop.toolFromJs(ToolWireInterop.toolToJs(richWit))
      assertTrue(roundtripped == richWit)
    },
    test("GOL-40 rich metadata represents the complete contract and exposes its invalid identifier") {
      val expected = gol40RichTool
      val root     = expected.commands(0)
      val render   = expected.commands(1)
      val status   = expected.commands(2)
      val body     = render.body.get
      assertTrue(
        expected.version == "1.0.0",
        !expected.requiresFilesystem,
        root.name == "artifact",
        root.aliases == List("art"),
        root.doc == Doc(
          "Build and inspect artifacts",
          "A deliberately asymmetric conformance tool.",
          List(Example("Render", "artifact --region eu-west-1 render src/main.wasm --format json"))
        ),
        root.globals.options.map(_.long) == List("region"),
        root.globals.flags.map(_.long) == List("trace"),
        root.body.isEmpty,
        render.aliases == List("build"),
        render.globals.options.map(_.long) == List("profile"),
        render.subcommands == List(2),
        status.aliases == List("show"),
        body.positionals.fixed.map(_.name) == List("request"),
        body.positionals.tail.map(t => (t.name, t.min, t.max, t.separator, t.verbatim)) ==
          Some(("inputs", 1, Some(3), Some("--"), true)),
        body.options.map(_.long) == List("format", "tag", "define", "color"),
        body.flags.map(_.long) == List("checksum", "verbose"),
        body.constraints.map(_.productPrefix) == List("RequiresAll", "Implies", "Forbids"),
        body.stdin.exists(_.mime == List("application/wasm")),
        body.stdout.exists(spec => spec.required && spec.mime == List("text/plain; charset=utf-8")),
        body.stderr.exists(_.mime == List("application/octet-stream")),
        body.result.exists(result =>
          result.formatters.map(formatter => formatter.name -> formatter.doc.summary) ==
            List("json" -> "JSON report", "table" -> "Tabular report") && result.defaultFormatter == "json"
        ),
        body.errors.map(error => (error.name, error.kind, error.exitCode, error.payload.isDefined)) == List(
          ("invalid-request", ErrorKind.UsageError, 2, true),
          ("render-failed", ErrorKind.RuntimeError, 70, true)
        ),
        body.annotations.contains(
          CommandAnnotations(readOnly = false, destructive = false, idempotent = true, openWorld = false)
        ),
        expected.canonicalInputFields(1).map(_.name) == List(
          "region",
          "trace",
          "profile",
          "request",
          "inputs",
          "format",
          "tag",
          "define",
          "color",
          "checksum",
          "verbose"
        ),
        expected.canonicalInputFields(2).map(_.name) == List("region", "trace", "profile", "artifact-id"),
        expected.tryToTool.isRight
      )
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
        WitToolError.CustomError(WitCustomToolError("failure", typed("boom")))
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
        WitToolError.CustomError(WitCustomToolError("failure", typed("x")))
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
