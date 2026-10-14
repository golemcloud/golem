/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.runtime.tool

import golem.FutureInterop
import golem.host.SchemaWireInterop
import golem.host.js.tool.JsInvocationResult
import golem.runtime.guest.Guest
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.schema.wire.SchemaWire
import golem.tool.*
import zio.ZIO
import zio.test.*

import scala.concurrent.Future
import scala.scalajs.js

object Gol40ProviderTerminalAcceptanceSpec extends ZIOSpecDefault {
  import ToolTestFixtures.*

  private enum StructuredOutcome {
    case Success
    case DeclaredError
    case Exception
  }

  private final case class Scenario(
    name: String,
    writerTerminal: Option[ByteStreamFailure],
    structuredOutcome: StructuredOutcome,
    finishFails: Boolean,
    expectedStdoutBytes: List[Byte],
    expectedStdoutTerminal: String,
    expectedStderrBytes: List[Byte],
    expectedStderrTerminal: String,
    expectedStructuredOutcome: String,
    expectedStdinCloses: Int,
    expectedOwnerOutcome: String
  )

  private final case class Attachments(
    stdin: ToolHostApi.RawByteStream,
    stdout: ToolHostApi.RawToolOutputWriter,
    stdinCloses: () => Int,
    bytes: () => List[Byte],
    terminal: () => Option[String],
    finishes: () => Int,
    failures: () => Int,
    disposals: () => Int
  )

  private val partialStdout = "partial-out".getBytes("UTF-8").toList
  private val partialStderr = List[Byte](0, 1, 2)

  private val scenarios = List(
    Scenario(
      "partial-success",
      None,
      StructuredOutcome.Success,
      false,
      partialStdout,
      "ended",
      partialStderr,
      "ended",
      "success",
      1,
      "resolved"
    ),
    Scenario(
      "declared-error",
      None,
      StructuredOutcome.DeclaredError,
      false,
      partialStdout,
      "ended",
      partialStderr,
      "ended",
      "custom-error",
      1,
      "rejected"
    ),
    Scenario(
      "explicit-failure-success",
      Some(ByteStreamFailure.Failed("provider-selected")),
      StructuredOutcome.Success,
      false,
      partialStdout,
      "failed:failed",
      partialStderr,
      "ended",
      "success",
      1,
      "resolved"
    ),
    Scenario(
      "exception",
      None,
      StructuredOutcome.Exception,
      false,
      partialStdout,
      "failed:failed",
      partialStderr,
      "failed:failed",
      "exception",
      1,
      "rejected"
    ),
    Scenario(
      "writer-abandoned",
      Some(ByteStreamFailure.Abandoned),
      StructuredOutcome.Success,
      false,
      partialStdout,
      "failed:abandoned",
      partialStderr,
      "ended",
      "success",
      1,
      "resolved"
    ),
    Scenario(
      "finish-failure-success",
      None,
      StructuredOutcome.Success,
      true,
      partialStdout,
      "ended",
      partialStderr,
      "ended",
      "success",
      1,
      "resolved"
    ),
    Scenario(
      "finish-failure-declared",
      None,
      StructuredOutcome.DeclaredError,
      true,
      partialStdout,
      "ended",
      partialStderr,
      "ended",
      "custom-error",
      1,
      "rejected"
    ),
    Scenario(
      "writer-cancelled",
      Some(ByteStreamFailure.Cancelled),
      StructuredOutcome.Success,
      false,
      partialStdout,
      "failed:cancelled",
      partialStderr,
      "ended",
      "success",
      1,
      "resolved"
    )
  )

  private def dualOutputTool(name: String): ExtendedToolType =
    ExtendedToolType(
      "1.0.0",
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
              Some(StreamSpec(doc(""), Nil, required = true)),
              None,
              List(
                ExtendedErrorCase(
                  "declared",
                  doc(""),
                  ErrorKind.RuntimeError,
                  1,
                  Some(strGraph)
                )
              ),
              None
            )
          )
        )
      )
    )

  private def invoker(tool: ExtendedToolType, scenario: Scenario): ToolRegistry.ToolInvoker = {
    val handle = ToolImplementationHandle(
      _ => Right(tool),
      List(
        ToolMethodBinding(
          tool.commands.head.name,
          Nil,
          ctx =>
            ToolInvokerRuntime.decodeArgs(ctx, List(ToolParamDecoder.StdoutParam, ToolParamDecoder.StderrParam)) match {
              case Left(error)                   => Future.successful(Left(error))
              case Right((args, stdout, stderr)) =>
                val out = args(0).asInstanceOf[ToolOutputStream]
                val err = args(1).asInstanceOf[ToolOutputStream]
                out
                  .write("partial-out".getBytes("UTF-8"))
                  .flatMap {
                    case Left(error) => Future.failed(new IllegalStateException(error.toString))
                    case Right(_)    =>
                      err
                        .write(Array[Byte](0, 1, 2))
                        .flatMap {
                          case Left(error) => Future.failed(new IllegalStateException(error.toString))
                          case Right(_)    =>
                            val terminal = scenario.writerTerminal match {
                              case Some(reason) => out.fail(reason)
                              case None         => Future.successful(Right(()): Either[StreamWriteError, Unit])
                            }
                            terminal.flatMap {
                              case Left(error) => Future.failed(new IllegalStateException(error.toString))
                              case Right(_)    =>
                                scenario.structuredOutcome match {
                                  case StructuredOutcome.Success =>
                                    Future.successful(ToolInvokerRuntime.encodeUnit(stdout, stderr))
                                  case StructuredOutcome.DeclaredError =>
                                    Future.successful(
                                      Left(
                                        ToolInvokeError.UnknownToolError(
                                          "declared",
                                          TypedSchemaValue(strGraph, SchemaValue.StringValue("declared-payload"))
                                        )
                                      )
                                    )
                                  case StructuredOutcome.Exception =>
                                    Future.failed(new RuntimeException("provider exception"))
                                }
                            }(ToolInvokerRuntime.executionContext)
                        }(ToolInvokerRuntime.executionContext)
                  }(ToolInvokerRuntime.executionContext)
            }
        )
      ),
      Nil
    )
    ToolImplementationRuntime.adaptHandler(tool, handle)
  }

  private def attachments(finishFails: Boolean): Attachments = {
    var stdinCloses = 0
    var bytes       = List.empty[Byte]
    var terminal    = Option.empty[String]
    var finishes    = 0
    var failures    = 0
    var disposals   = 0
    val done        = js.Dynamic.literal("done" -> true, "value" -> js.undefined)

    def resolved(value: js.Any): js.Promise[js.Any] =
      js.Dynamic.global.Promise.resolve(value).asInstanceOf[js.Promise[js.Any]]

    val iterator = js.Dynamic.literal(
      "next" -> js.Any.fromFunction0(() => resolved(done.asInstanceOf[js.Any]))
    )
    iterator.updateDynamic("return")(
      js.Any.fromFunction0 { () =>
        stdinCloses += 1
        resolved(done.asInstanceOf[js.Any])
      }
    )
    val rawStdin = js.Dynamic.literal()
    js.Dynamic.global.Reflect.set(rawStdin, js.Symbol.asyncIterator, js.Any.fromFunction0(() => iterator))

    val rawStdout = js.Dynamic.literal(
      "write" -> js.Any.fromFunction1 { (chunk: js.typedarray.Uint8Array) =>
        bytes = bytes ++ chunk.toArray.map(_.toByte)
        js.Promise.resolve[Unit](())
      },
      "finish" -> js.Any.fromFunction0 { () =>
        finishes += 1
        terminal = Some("ended")
        if (finishFails) js.Promise.reject(new RuntimeException("finish failed")) else js.Promise.resolve[Unit](())
      },
      "fail" -> js.Any.fromFunction1 { (reason: js.Dynamic) =>
        failures += 1
        terminal = Some(s"failed:${reason.tag.asInstanceOf[String]}")
        js.Promise.resolve[Unit](())
      }
    )
    js.Dynamic.global.Reflect.set(
      rawStdout,
      js.Dynamic.global.Symbol.selectDynamic("dispose"),
      js.Any.fromFunction0(() => disposals += 1)
    )

    Attachments(
      rawStdin.asInstanceOf[ToolHostApi.RawByteStream],
      rawStdout.asInstanceOf[ToolHostApi.RawToolOutputWriter],
      () => stdinCloses,
      () => bytes,
      () => terminal,
      () => finishes,
      () => failures,
      () => disposals
    )
  }

  private def emptyInput(tool: ExtendedToolType): js.Any =
    SchemaWireInterop.typedToJs(
      SchemaWire.typedSchemaValueToWit(
        TypedSchemaValue(tool.canonicalInputRecordSchema(0).toOption.get, SchemaValue.RecordValue(Nil))
      )
    )

  private def invoke(tool: ExtendedToolType, stdin: Attachments, stdout: Attachments, stderr: Attachments) =
    Guest.golemTool010Guest
      .invoke(
        tool.toolName,
        js.Array[String](),
        emptyInput(tool),
        stdin.stdin,
        stdout.stdout,
        stderr.stdout,
        js.Dynamic.literal("tag" -> "anonymous")
      )
      .asInstanceOf[js.Promise[JsInvocationResult]]

  override def spec = suite("GOL-40 provider terminal acceptance")(
    test("SDK-TERMINALS row 7 maps bytes terminals structured outcome stdin cleanup and owner outcome") {
      ZIO
        .foreach(scenarios.zipWithIndex) { case (scenario, index) =>
          val tool   = dualOutputTool(s"gol40-provider-terminal-${scenario.name}-$index")
          val stdin  = attachments(finishFails = false)
          val stdout = attachments(scenario.finishFails)
          val stderr = attachments(finishFails = false)
          ToolRegistry.registerInvoker(tool, invoker(tool, scenario))

          ZIO.fromFuture(_ => FutureInterop.fromPromise(invoke(tool, stdin, stdout, stderr))).either.map { result =>
            val ownerOutcome      = result.fold(_ => "rejected", _ => "resolved")
            val structuredOutcome = result match {
              case Right(_)                          => "success"
              case Left(js.JavaScriptException(raw)) =>
                val dynamic = raw.asInstanceOf[js.Dynamic]
                if (js.typeOf(dynamic.selectDynamic("tag")) == "string") dynamic.tag.asInstanceOf[String]
                else "exception"
              case Left(_) => "exception"
            }

            assertTrue(
              structuredOutcome == scenario.expectedStructuredOutcome,
              ownerOutcome == scenario.expectedOwnerOutcome,
              stdout.bytes() == scenario.expectedStdoutBytes,
              stdout.terminal().contains(scenario.expectedStdoutTerminal),
              stderr.bytes() == scenario.expectedStderrBytes,
              stderr.terminal().contains(scenario.expectedStderrTerminal),
              stdin.stdinCloses() == scenario.expectedStdinCloses,
              stdout.disposals() == 1,
              stderr.disposals() == 1,
              stdout.finishes() + stdout.failures() == 1,
              stderr.finishes() == (if (scenario.expectedStderrTerminal == "ended") 1 else 0),
              stderr.failures() == (if (scenario.expectedStderrTerminal.startsWith("failed:")) 1 else 0)
            ).label(scenario.name)
          }
        }
        .map(_.reduce(_ && _))
    }
  ) @@ TestAspect.sequential
}
