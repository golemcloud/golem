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

import golem.host.SchemaWireInterop
import golem.host.js.tool.{JsInvocationResult, JsTool}
import golem.runtime.guest.Guest
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
import golem.tool._
import golem.tool.wire.WitToolError
import golem.{FutureInterop, Principal}
import zio.test._
import zio.ZIO

import scala.concurrent.Future
import scala.scalajs.js

/**
 * Drives the tool registry through the exported `golemTool010Guest` object,
 * i.e. the `golem:tool/guest@0.1.0` boundary: discover / get / invoke with
 * JS-encoded tools, inputs, results, and `tool-error` rejections.
 */
object ToolGuestSpec extends ZIOSpecDefault {
  import ToolTestFixtures._

  private def guest: js.Dynamic = Guest.golemTool010Guest

  private val anonymousPrincipal: js.Dynamic = js.Dynamic.literal("tag" -> "anonymous")

  /** An absent `stdin` parameter, pre-typed for `js.Dynamic` application. */
  private val noStdin: js.Any  = js.undefined.asInstanceOf[js.Any]
  private val noStdout: js.Any = js.undefined.asInstanceOf[js.Any]

  private def typed(s: String): WitTypedSchemaValue =
    SchemaWire.typedSchemaValueToWit(TypedSchemaValue(strGraph, SchemaValue.StringValue(s)))

  private def fromPromise[A](p: js.Promise[A]): ZIO[Any, Throwable, A] =
    ZIO.fromFuture(implicit ec => FutureInterop.fromPromise(p))

  /**
   * Runs the promise, expecting a rejection carrying a `{ tag, val }`
   * tool-error.
   */
  private def rejectionOf[A](p: js.Promise[A]): ZIO[Any, Nothing, js.Dynamic] =
    fromPromise(p).flip.orDieWith(_ => new RuntimeException("expected the promise to be rejected")).map {
      case js.JavaScriptException(e) => e.asInstanceOf[js.Dynamic]
      case other                     => throw other
    }

  // --- Fixture registrations (once per module) -------------------------------

  private lazy val discoverToolsRegistered: Unit = {
    ToolRegistry.register(leafTool("guest-zeta"))
    ToolRegistry.register(leafTool("guest-alpha"))
  }

  private final class Captured {
    var commandPath: List[String] = Nil
    var principal: Principal      = Principal.Anonymous
    var stdinPresent: Boolean     = true
  }

  private lazy val echoCaptured: Captured = {
    val captured                          = new Captured
    val invoker: ToolRegistry.ToolInvoker = (path, input, stdin, _, principal) => {
      captured.commandPath = path
      captured.principal = principal
      captured.stdinPresent = stdin.isDefined
      Future.successful(Right(ToolInvocationResult(Some(input))))
    }
    ToolRegistry.registerInvoker(echoTool("guest-echo"), invoker)
    captured
  }

  private lazy val failingRegistered: Unit = {
    val invoker: ToolRegistry.ToolInvoker = (_, _, _, _, _) =>
      Future.successful(Left(WitToolError.CustomError(typed("boom"))))
    ToolRegistry.registerInvoker(echoTool("guest-failing"), invoker)
  }

  private lazy val definitionOnlyRegistered: Unit =
    ToolRegistry.register(leafTool("guest-definition-only"))

  private def stdoutTool(name: String): ExtendedToolType =
    ExtendedToolType(
      "0.1.0",
      Vector(
        ExtendedCommandNode(
          name,
          Nil,
          doc(""),
          ExtendedGlobals.empty,
          Nil,
          Some(
            ExtendedCommandBody(
              ExtendedPositionals.empty,
              Nil,
              Nil,
              Nil,
              None,
              Some(StreamSpec(doc(""), Nil, required = true)),
              None,
              Nil,
              None
            )
          )
        )
      )
    )

  private def stdoutInvoker(
    tool: ExtendedToolType,
    outcome: Either[ToolInvokeError, ToolInvokeResult]
  ): ToolRegistry.ToolInvoker = {
    val handle = ToolImplementationHandle(
      _ => Right(tool),
      List(ToolMethodBinding(tool.commands.head.name, Nil, _ => Future.successful(outcome))),
      Nil
    )
    ToolImplementationRuntime.adaptHandler(tool, handle)
  }

  private def emptyInput(tool: ExtendedToolType): WitTypedSchemaValue =
    SchemaWire.typedSchemaValueToWit(
      TypedSchemaValue(
        tool.canonicalInputRecordSchema(0).toOption.get,
        SchemaValue.RecordValue(Nil)
      )
    )

  private def stdoutWriter(onFinish: () => Unit): ToolHostApi.RawToolStdoutWriter =
    js.Dynamic
      .literal(
        "write"  -> js.Any.fromFunction1((_: js.typedarray.Uint8Array) => js.Promise.resolve[Unit](())),
        "finish" -> js.Any.fromFunction0 { () =>
          onFinish()
          js.Promise.resolve[Unit](())
        },
        "fail" -> js.Any.fromFunction1((_: js.Any) => js.Promise.resolve[Unit](()))
      )
      .asInstanceOf[ToolHostApi.RawToolStdoutWriter]

  def spec: Spec[Any, Any] = suite("ToolGuestSpec")(
    test("discover_tools_returns_registered_tools_sorted_by_name") {
      discoverToolsRegistered
      val tools = guest.discoverTools().asInstanceOf[js.Array[JsTool]]
      val names = tools.toList.map(t => ToolWireInteropAccess.rootName(t))
      assertTrue(
        names.contains("guest-alpha"),
        names.contains("guest-zeta"),
        names == names.sorted,
        names.indexOf("guest-alpha") < names.indexOf("guest-zeta")
      )
    },
    test("get_tool_returns_the_wire_descriptor") {
      discoverToolsRegistered
      val tool = guest.getTool("guest-alpha").asInstanceOf[JsTool]
      assertTrue(
        golem.host.ToolWireInterop.toolFromJs(tool) == leafTool("guest-alpha").toTool
      )
    },
    test("get_tool_rejects_unknown_names_with_invalid_tool_name") {
      val err =
        try {
          guest.getTool("guest-nope")
          throw new RuntimeException("expected getTool to throw")
        } catch {
          case js.JavaScriptException(error) => error.asInstanceOf[js.Dynamic]
        }
      assertTrue(
        err.tag.asInstanceOf[String] == "invalid-tool-name",
        err.selectDynamic("val").asInstanceOf[String] == "guest-nope"
      )
    },
    test("invoke_dispatches_to_the_registered_invoker_and_roundtrips_the_result") {
      val captured = echoCaptured
      val input    = SchemaWireInterop.typedToJs(typed("hello"))
      for {
        res <- fromPromise(
                 guest
                   .invoke("guest-echo", js.Array[String](), input, noStdin, noStdout, anonymousPrincipal)
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
        result = res.result.toOption.map(SchemaWireInterop.typedFromJs)
      } yield assertTrue(
        result.contains(typed("hello")),
        captured.commandPath == Nil,
        captured.principal == Principal.Anonymous,
        !captured.stdinPresent
      )
    },
    test("invoke_passes_the_command_path_to_the_invoker") {
      val captured = echoCaptured
      val input    = SchemaWireInterop.typedToJs(typed("deep"))
      for {
        _ <- fromPromise(
               guest
                 .invoke("guest-echo", js.Array("sub", "leaf"), input, noStdin, noStdout, anonymousPrincipal)
                 .asInstanceOf[js.Promise[JsInvocationResult]]
             )
      } yield assertTrue(captured.commandPath == List("sub", "leaf"))
    },
    test("invoke_rejects_unknown_tools_with_invalid_tool_name") {
      val input = SchemaWireInterop.typedToJs(typed("x"))
      for {
        err <- rejectionOf(
                 guest
                   .invoke("guest-nope", js.Array[String](), input, noStdin, noStdout, anonymousPrincipal)
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
      } yield assertTrue(
        err.tag.asInstanceOf[String] == "invalid-tool-name",
        err.selectDynamic("val").asInstanceOf[String] == "guest-nope"
      )
    },
    test("invoke_rejects_definition_only_tools_with_invalid_tool_name") {
      definitionOnlyRegistered
      val input = SchemaWireInterop.typedToJs(typed("x"))
      for {
        err <- rejectionOf(
                 guest
                   .invoke("guest-definition-only", js.Array[String](), input, noStdin, noStdout, anonymousPrincipal)
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
      } yield assertTrue(err.tag.asInstanceOf[String] == "invalid-tool-name")
    },
    test("invoke_encodes_custom_errors_as_typed_schema_values") {
      failingRegistered
      val input = SchemaWireInterop.typedToJs(typed("x"))
      for {
        err <- rejectionOf(
                 guest
                   .invoke("guest-failing", js.Array[String](), input, noStdin, noStdout, anonymousPrincipal)
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
        payload = SchemaWireInterop.typedFromJs(
                    err.selectDynamic("val").asInstanceOf[golem.host.js.schema.JsTypedSchemaValue]
                  )
      } yield assertTrue(
        err.tag.asInstanceOf[String] == "custom-error",
        payload == typed("boom")
      )
    },
    test("provider invocation default-finishes stdout after structured success") {
      val tool     = stdoutTool("guest-stdout-success")
      var finishes = 0
      val invoked  = stdoutInvoker(tool, Right(ToolInvokeResult(None)))(
        Nil,
        emptyInput(tool),
        None,
        Some(stdoutWriter(() => finishes += 1)),
        Principal.Anonymous
      )
      ZIO.fromFuture(_ => invoked).map { result =>
        assertTrue(result == Right(ToolInvocationResult(None)), finishes == 1)
      }
    },
    test("provider invocation default-finishes stdout after a declared error") {
      val tool     = stdoutTool("guest-stdout-error")
      var finishes = 0
      val invoked  = stdoutInvoker(
        tool,
        Left(ToolInvokeError.Custom(TypedSchemaValue(strGraph, SchemaValue.StringValue("boom"))))
      )(
        Nil,
        emptyInput(tool),
        None,
        Some(stdoutWriter(() => finishes += 1)),
        Principal.Anonymous
      )
      ZIO.fromFuture(_ => invoked).map { result =>
        assertTrue(result.isLeft, finishes == 1)
      }
    },
    test("invoke_rejects_malformed_input_with_invalid_input") {
      val captured = echoCaptured
      val _        = captured
      for {
        err <- rejectionOf(
                 guest
                   .invoke(
                     "guest-echo",
                     js.Array[String](),
                     js.Dynamic.literal("graph" -> js.Dynamic.literal()),
                     js.undefined,
                     js.undefined,
                     anonymousPrincipal
                   )
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
      } yield assertTrue(err.tag.asInstanceOf[String] == "invalid-input")
    }
  ) @@ TestAspect.sequential
}

/** Test-side helper to read the root command name off a JS tool facade. */
private object ToolWireInteropAccess {
  def rootName(tool: JsTool): String =
    tool.commands.nodes(0).name
}
