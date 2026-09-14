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
import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool.{JsInvocationResult, JsTool}
import golem.runtime.guest.Guest
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{AgentStream, IntoSchema, SchemaValue, TypedSchemaValue}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
import golem.tool._
import golem.tool.wire.WitToolError
import golem.{FutureInterop, Principal}
import zio.test._
import zio.ZIO

import scala.concurrent.{Future, Promise}
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
    outcome: Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]
  ): ToolRegistry.ToolInvoker = {
    val handle = ToolImplementationHandle(
      _ => Right(tool),
      List(ToolMethodBinding(tool.commands.head.name, Nil, _ => Future.successful(outcome))),
      Nil
    )
    ToolImplementationRuntime.adaptHandler(tool, handle)
  }

  private def attachmentTool(name: String): ExtendedToolType =
    stdoutTool(name).copy(commands = stdoutTool(name).commands.map { node =>
      node.copy(body = node.body.map(_.copy(stdin = Some(StreamSpec(doc(""), Nil, required = true)))))
    })

  private def attachmentInvoker(tool: ExtendedToolType): ToolRegistry.ToolInvoker = {
    val handle = ToolImplementationHandle(
      _ => Right(tool),
      List(
        ToolMethodBinding(
          tool.commands.head.name,
          Nil,
          ctx =>
            ToolInvokerRuntime.decodeArgs(
              ctx,
              List(ToolParamDecoder.StdinParam, ToolParamDecoder.StdoutParam)
            ) match {
              case Left(error)              => Future.successful(Left(error))
              case Right((_, stdoutHandle)) => Future.successful(ToolInvokerRuntime.encodeUnit(stdoutHandle))
            }
        )
      ),
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

  private final case class InvocationAttachments(
    stdin: ToolHostApi.RawByteStream,
    stdout: ToolHostApi.RawToolStdoutWriter,
    stdinCloses: () => Int,
    stdoutFinishes: () => Int
  )

  private def invocationAttachments(cleanupFails: Boolean = false): InvocationAttachments = {
    var stdinCloses                                 = 0
    var stdoutFinishes                              = 0
    val done                                        = js.Dynamic.literal("done" -> true, "value" -> js.undefined)
    def resolved(value: js.Any): js.Promise[js.Any] =
      js.Dynamic.global.Promise.resolve(value).asInstanceOf[js.Promise[js.Any]]
    def cleanup(value: js.Any): js.Promise[js.Any] =
      if (cleanupFails) js.Promise.reject(new RuntimeException("cleanup failed")).asInstanceOf[js.Promise[js.Any]]
      else resolved(value)
    val iterator = js.Dynamic.literal(
      "next" -> js.Any.fromFunction0(() => resolved(done.asInstanceOf[js.Any]))
    )
    iterator.updateDynamic("return")(
      js.Any.fromFunction0 { () =>
        stdinCloses += 1
        cleanup(done.asInstanceOf[js.Any])
      }
    )
    val rawStdin = js.Dynamic.literal()
    js.Dynamic.global.Reflect.set(
      rawStdin,
      js.Symbol.asyncIterator,
      js.Any.fromFunction0(() => iterator)
    )
    val rawStdout = js.Dynamic.literal(
      "write"  -> js.Any.fromFunction1((_: js.typedarray.Uint8Array) => js.Promise.resolve[Unit](())),
      "finish" -> js.Any.fromFunction0 { () =>
        stdoutFinishes += 1
        cleanup(js.undefined)
      },
      "fail" -> js.Any.fromFunction1((_: js.Any) => js.Promise.resolve[Unit](()))
    )
    InvocationAttachments(
      rawStdin.asInstanceOf[ToolHostApi.RawByteStream],
      rawStdout.asInstanceOf[ToolHostApi.RawToolStdoutWriter],
      () => stdinCloses,
      () => stdoutFinishes
    )
  }

  private def invokeAtGuest(
    toolName: String,
    input: js.Any,
    stdin: Option[ToolHostApi.RawByteStream],
    stdout: Option[ToolHostApi.RawToolStdoutWriter],
    commandPath: js.Array[String] = js.Array[String]()
  ): js.Promise[JsInvocationResult] =
    guest
      .invoke(
        toolName,
        commandPath,
        input,
        stdin.fold[js.Any](js.undefined)(_.asInstanceOf[js.Any]),
        stdout.fold[js.Any](js.undefined)(_.asInstanceOf[js.Any]),
        anonymousPrincipal
      )
      .asInstanceOf[js.Promise[JsInvocationResult]]

  private def encodedInput(input: WitTypedSchemaValue): js.Any =
    SchemaWireInterop.typedToJs(input).asInstanceOf[js.Any]

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
    test("early invalid-tool rejection closes stdin and finishes stdout") {
      val attachments = invocationAttachments()
      for {
        err <- rejectionOf(
                 invokeAtGuest(
                   "guest-nope-with-attachments",
                   encodedInput(typed("x")),
                   Some(attachments.stdin),
                   Some(attachments.stdout)
                 )
               )
      } yield assertTrue(
        err.tag.asInstanceOf[String] == "invalid-tool-name",
        attachments.stdinCloses() == 1,
        attachments.stdoutFinishes() == 1
      )
    },
    test("early invalid-tool rejection closes schema streams carried by input") {
      val into   = IntoSchema[AgentStream[String]]
      var closes = 0
      val source = AgentStream.fromPull[String](
        () => Future.successful(None),
        () => {
          closes += 1
          Future.successful(())
        }
      )
      val input = SchemaWire.typedSchemaValueToWit(into.toTyped(source))
      ZIO.fromFuture { implicit ec =>
        for {
          value  <- SchemaWireInterop.valueTreeToJsAsync(input.value)
          encoded = JsTypedSchemaValue(SchemaWireInterop.graphToJs(input.graph), value)
          error  <- FutureInterop
                     .fromPromise(
                       invokeAtGuest("guest-nope-with-stream-input", encoded, None, None)
                     )
                     .failed
        } yield assertTrue(
          error.isInstanceOf[js.JavaScriptException],
          closes == 1
        )
      }
    },
    test("invalid command-path rejection closes invocation attachments") {
      val tool        = stdoutTool("guest-invalid-command-path")
      val attachments = invocationAttachments()
      ToolRegistry.registerInvoker(tool, stdoutInvoker(tool, Right(ToolInvokeResult(None))))
      for {
        error <- rejectionOf(
                   invokeAtGuest(
                     tool.toolName,
                     encodedInput(emptyInput(tool)),
                     Some(attachments.stdin),
                     Some(attachments.stdout),
                     js.Array("missing")
                   )
                 )
      } yield assertTrue(
        error.tag.asInstanceOf[String] == "invalid-command-path",
        attachments.stdinCloses() == 1,
        attachments.stdoutFinishes() == 1
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
      val tool        = stdoutTool("guest-stdout-success")
      val attachments = invocationAttachments()
      ToolRegistry.registerInvoker(tool, stdoutInvoker(tool, Right(ToolInvokeResult(None))))
      fromPromise(
        invokeAtGuest(
          tool.toolName,
          encodedInput(emptyInput(tool)),
          None,
          Some(attachments.stdout)
        )
      ).map { result =>
        val hasNoResult = result.result.toOption.isEmpty
        assertTrue(hasNoResult, attachments.stdoutFinishes() == 1)
      }
    },
    test("provider invocation default-finishes stdout after a declared error") {
      val tool        = stdoutTool("guest-stdout-error")
      val attachments = invocationAttachments()
      ToolRegistry.registerInvoker(
        tool,
        stdoutInvoker(
          tool,
          Left(ToolInvokeError.Tool(TypedSchemaValue(strGraph, SchemaValue.StringValue("boom"))))
        )
      )
      for {
        error <- rejectionOf(
                   invokeAtGuest(
                     tool.toolName,
                     encodedInput(emptyInput(tool)),
                     Some(attachments.stdin),
                     Some(attachments.stdout)
                   )
                 )
      } yield assertTrue(
        error.tag.asInstanceOf[String] == "custom-error",
        attachments.stdinCloses() == 1,
        attachments.stdoutFinishes() == 1
      )
    },
    test("accepted attachments stay live through the invocation and close exactly once") {
      val tool                              = echoTool("guest-attachment-transfer")
      val attachments                       = invocationAttachments()
      val completed                         = Promise[Either[WitToolError, ToolInvocationResult]]()
      var acceptedIn                        = Option.empty[ToolInputStream]
      var acceptedOut                       = Option.empty[ToolOutputStream]
      val invoker: ToolRegistry.ToolInvoker = (_, _, stdin, stdout, _) => {
        acceptedIn = stdin
        acceptedOut = stdout
        completed.future
      }
      ToolRegistry.registerInvoker(tool, invoker)
      val invocation = fromPromise(
        invokeAtGuest(
          tool.toolName,
          encodedInput(emptyInput(tool)),
          Some(attachments.stdin),
          Some(attachments.stdout)
        )
      )
      for {
        _ <- ZIO.succeed(
               assertTrue(
                 acceptedIn.nonEmpty,
                 acceptedOut.nonEmpty,
                 attachments.stdinCloses() == 0,
                 attachments.stdoutFinishes() == 0
               )
             )
        _ <- ZIO.fromFuture(_ => acceptedIn.get.cancel())
        _ <- ZIO.fromFuture(_ => acceptedOut.get.finish())
        _ <- ZIO.succeed(completed.success(Right(ToolInvocationResult(None))))
        _ <- invocation
      } yield assertTrue(
        attachments.stdinCloses() == 1,
        attachments.stdoutFinishes() == 1
      )
    },
    test("missing declared attachments release every supplied peer") {
      val missingStdinTool = attachmentTool("guest-missing-stdin")
      val missingStdin     = invocationAttachments()
      ToolRegistry.registerInvoker(missingStdinTool, attachmentInvoker(missingStdinTool))
      val missingStdoutTool = attachmentTool("guest-missing-stdout")
      val missingStdout     = invocationAttachments()
      ToolRegistry.registerInvoker(missingStdoutTool, attachmentInvoker(missingStdoutTool))
      for {
        stdinError <- rejectionOf(
                        invokeAtGuest(
                          missingStdinTool.toolName,
                          encodedInput(emptyInput(missingStdinTool)),
                          None,
                          Some(missingStdin.stdout)
                        )
                      )
        stdoutError <- rejectionOf(
                         invokeAtGuest(
                           missingStdoutTool.toolName,
                           encodedInput(emptyInput(missingStdoutTool)),
                           Some(missingStdout.stdin),
                           None
                         )
                       )
      } yield assertTrue(
        stdinError.tag.asInstanceOf[String] == "invalid-input",
        missingStdin.stdoutFinishes() == 1,
        stdoutError.tag.asInstanceOf[String] == "invalid-input",
        missingStdout.stdinCloses() == 1
      )
    },
    test("cleanup failures do not mask synchronous or asynchronous invocation failures") {
      val syncTool    = echoTool("guest-sync-failure")
      val syncFailure = new RuntimeException("synchronous failure")
      ToolRegistry.registerInvoker(syncTool, (_, _, _, _, _) => throw syncFailure)
      val syncAttachments = invocationAttachments(cleanupFails = true)
      val asyncTool       = echoTool("guest-async-failure")
      val asyncFailure    = new RuntimeException("asynchronous failure")
      ToolRegistry.registerInvoker(asyncTool, (_, _, _, _, _) => Future.failed(asyncFailure))
      val asyncAttachments = invocationAttachments(cleanupFails = true)
      for {
        syncError <- fromPromise(
                       invokeAtGuest(
                         syncTool.toolName,
                         encodedInput(emptyInput(syncTool)),
                         Some(syncAttachments.stdin),
                         Some(syncAttachments.stdout)
                       )
                     ).flip
        asyncError <- fromPromise(
                        invokeAtGuest(
                          asyncTool.toolName,
                          encodedInput(emptyInput(asyncTool)),
                          Some(asyncAttachments.stdin),
                          Some(asyncAttachments.stdout)
                        )
                      ).flip
      } yield assertTrue(
        syncError.getMessage.contains(syncFailure.getMessage),
        syncAttachments.stdinCloses() == 1,
        syncAttachments.stdoutFinishes() == 1,
        asyncError.getMessage.contains(asyncFailure.getMessage),
        asyncAttachments.stdinCloses() == 1,
        asyncAttachments.stdoutFinishes() == 1
      )
    },
    test("malformed input releases invocation attachments before returning invalid-input") {
      val captured    = echoCaptured
      val _           = captured
      val attachments = invocationAttachments()
      for {
        err <- rejectionOf(
                 invokeAtGuest(
                   "guest-echo",
                   js.Dynamic.literal("graph" -> js.Dynamic.literal()),
                   Some(attachments.stdin),
                   Some(attachments.stdout)
                 )
               )
      } yield assertTrue(
        err.tag.asInstanceOf[String] == "invalid-input",
        attachments.stdinCloses() == 1,
        attachments.stdoutFinishes() == 1
      )
    },
    test("provider structured success survives attachment cleanup failure") {
      val tool        = stdoutTool("guest-cleanup-failure-success")
      val attachments = invocationAttachments(cleanupFails = true)
      ToolRegistry.registerInvoker(tool, stdoutInvoker(tool, Right(ToolInvokeResult(None))))
      fromPromise(
        invokeAtGuest(
          tool.toolName,
          encodedInput(emptyInput(tool)),
          Some(attachments.stdin),
          Some(attachments.stdout)
        )
      ).map { result =>
        val hasNoResult = result.result.toOption.isEmpty
        assertTrue(
          hasNoResult,
          attachments.stdinCloses() == 1,
          attachments.stdoutFinishes() == 1
        )
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
