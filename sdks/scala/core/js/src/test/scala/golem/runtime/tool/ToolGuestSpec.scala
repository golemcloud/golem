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
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
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
  private val noStdin: js.Any = js.undefined.asInstanceOf[js.Any]

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
    val invoker: ToolRegistry.ToolInvoker = (path, input, stdin, principal) => {
      captured.commandPath = path
      captured.principal = principal
      captured.stdinPresent = stdin.isDefined
      Future.successful(Right(ToolInvocationResult(Some(input), None)))
    }
    ToolRegistry.registerInvoker(echoTool("guest-echo"), invoker)
    captured
  }

  private lazy val failingRegistered: Unit = {
    val invoker: ToolRegistry.ToolInvoker = (_, _, _, _) =>
      Future.successful(Left(WitToolError.CustomError(typed("boom"))))
    ToolRegistry.registerInvoker(echoTool("guest-failing"), invoker)
  }

  private lazy val definitionOnlyRegistered: Unit =
    ToolRegistry.register(leafTool("guest-definition-only"))

  def spec: Spec[Any, Any] = suite("ToolGuestSpec")(
    test("discover_tools_returns_registered_tools_sorted_by_name") {
      discoverToolsRegistered
      for {
        tools <- fromPromise(guest.discoverTools().asInstanceOf[js.Promise[js.Array[JsTool]]])
        names  = tools.toList.map(t => ToolWireInteropAccess.rootName(t))
      } yield assertTrue(
        names.contains("guest-alpha"),
        names.contains("guest-zeta"),
        names == names.sorted,
        names.indexOf("guest-alpha") < names.indexOf("guest-zeta")
      )
    },
    test("get_tool_returns_the_wire_descriptor") {
      discoverToolsRegistered
      for {
        tool <- fromPromise(guest.getTool("guest-alpha").asInstanceOf[js.Promise[JsTool]])
      } yield assertTrue(
        golem.host.ToolWireInterop.toolFromJs(tool) == leafTool("guest-alpha").toTool
      )
    },
    test("get_tool_rejects_unknown_names_with_invalid_tool_name") {
      for {
        err <- rejectionOf(guest.getTool("guest-nope").asInstanceOf[js.Promise[JsTool]])
      } yield assertTrue(
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
                   .invoke("guest-echo", js.Array[String](), input, noStdin, anonymousPrincipal)
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
        result      = res.result.toOption.map(SchemaWireInterop.typedFromJs)
        stdoutEmpty = res.stdout.isEmpty
      } yield assertTrue(
        result.contains(typed("hello")),
        stdoutEmpty,
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
                 .invoke("guest-echo", js.Array("sub", "leaf"), input, noStdin, anonymousPrincipal)
                 .asInstanceOf[js.Promise[JsInvocationResult]]
             )
      } yield assertTrue(captured.commandPath == List("sub", "leaf"))
    },
    test("invoke_rejects_unknown_tools_with_invalid_tool_name") {
      val input = SchemaWireInterop.typedToJs(typed("x"))
      for {
        err <- rejectionOf(
                 guest
                   .invoke("guest-nope", js.Array[String](), input, noStdin, anonymousPrincipal)
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
                   .invoke("guest-definition-only", js.Array[String](), input, noStdin, anonymousPrincipal)
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
                   .invoke("guest-failing", js.Array[String](), input, noStdin, anonymousPrincipal)
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
                     anonymousPrincipal
                   )
                   .asInstanceOf[js.Promise[JsInvocationResult]]
               )
      } yield assertTrue(err.tag.asInstanceOf[String] == "invalid-input")
    }
  )
}

/** Test-side helper to read the root command name off a JS tool facade. */
private object ToolWireInteropAccess {
  def rootName(tool: JsTool): String =
    tool.commands.nodes(0).name
}
