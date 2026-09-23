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

import golem.schema.{FromSchema, IntoSchema, SchemaValue, TypedSchemaValue}
import golem.tool.ToolDeclaredErrorDecoder.{DeclaredErrors, NoDeclaredErrors}
import zio.ZIO
import zio.test._

import scala.concurrent.{Future, Promise}

object ToolCallBackendSpec extends ZIOSpecDefault {

  private val unitInput = ToolErrorSupport.unitPayload

  private def doc: Doc = Doc("", "", Nil)

  private def nestedDescriptor: ExtendedToolType = {
    val config = ExtendedOptionSpec(
      "config",
      None,
      Nil,
      doc,
      None,
      ExtendedOptionShape.Scalar(implicitly[IntoSchema[String]].graph),
      None,
      required = false,
      None
    )
    val verbose = FlagSpec(
      "verbose",
      None,
      Nil,
      doc,
      FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = false)),
      None
    )
    val body = ExtendedCommandBody(ExtendedPositionals.empty, Nil, Nil, Nil, None, None, None, Nil, None)
    ExtendedToolType(
      "0.1.0",
      Vector(
        ExtendedCommandNode("root", Nil, doc, ExtendedGlobals.empty, List(1), None),
        ExtendedCommandNode("child", Nil, doc, ExtendedGlobals(List(config), List(verbose)), List(2), None),
        ExtendedCommandNode("run", Nil, doc, ExtendedGlobals.empty, Nil, Some(body))
      )
    )
  }

  private final class EmptyStream extends ToolInputStream

  private final class FakeTransport(
    started: Either[ToolRpcFailure, ToolRpcStarted]
  ) extends ToolRpcTransport {
    var calls: Int = 0

    def start(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream],
      stdout: Boolean
    ): Either[ToolRpcFailure, ToolRpcStarted] = {
      calls += 1
      started
    }
  }

  private def prepared[E, A](
    errors: ToolDeclaredErrorDecoder[E],
    decode: Option[TypedSchemaValue] => Either[String, A],
    input: Either[ToolInvokeError[Nothing], TypedSchemaValue] = Right(unitInput)
  ): PreparedToolCall[ToolInputStream, E, A] =
    PreparedToolCall(List("run"), input, None, errors, decode)

  private def success(
    value: Option[TypedSchemaValue] = None,
    stdout: Option[ToolInputStream] = None,
    terminal: Option[Future[Either[ToolRpcFailure, ToolInvokeResult]]] = None
  ): ToolRpcStarted =
    ToolRpcStarted(stdout, terminal.getOrElse(Future.successful(Right(ToolInvokeResult(value)))), () => ())

  def spec: Spec[Any, Any] = suite("ToolCallBackendSpec")(
    test("awaited calls decode declared custom errors through the ambient policy") {
      val payload = implicitly[IntoSchema[String]].toTyped("bad input")
      val remote  = ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError("usage", payload))
      val backend = new AmbientToolCallBackend(
        new FakeTransport(Right(success(terminal = Some(Future.successful(Left(remote))))))
      )
      val call = prepared[String, Unit](
        DeclaredErrors(error =>
          if (error.name == "usage")
            implicitly[FromSchema[String]].fromValue(error.payload.value).left.map(_.message)
          else Left("unknown error")
        ),
        _ => Right(())
      )

      ZIO
        .fromFuture(_ => backend.awaitNoStdout(call))
        .map(result => assertTrue(result == Left(ToolError.Tool("bad input"))))
    },
    test("infallible awaited calls retain unknown remote tool errors") {
      val payload = implicitly[IntoSchema[String]].toTyped("unexpected")
      val remote  = ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError("future", payload))
      val backend = new AmbientToolCallBackend(
        new FakeTransport(Right(success(terminal = Some(Future.successful(Left(remote))))))
      )

      ZIO.fromFuture(_ => backend.awaitNoStdout(prepared(NoDeclaredErrors, _ => Right(())))).map {
        case Left(ToolError.RemoteTool(ToolInvokeError.UnknownToolError(name, actualPayload))) =>
          assertTrue(name == "future", actualPayload == payload)
        case other => assertNever(s"expected unknown remote tool error, got $other")
      }
    },
    test("stdout-bearing calls expose stdout before terminal completion") {
      val terminal = Promise[Either[ToolRpcFailure, ToolInvokeResult]]()
      val stream   = new EmptyStream
      val backend  = new AmbientToolCallBackend(
        new FakeTransport(Right(success(stdout = Some(stream), terminal = Some(terminal.future))))
      )
      val started = backend.startStdoutOnly(
        prepared(
          NoDeclaredErrors,
          {
            case None    => Right(())
            case Some(_) => Left("unexpected value")
          }
        )
      )

      started match {
        case Right(invocation) =>
          val exposedBeforeCompletion = (invocation.stdout eq stream) && !invocation.result.isCompleted
          terminal.success(Right(ToolInvokeResult(None)))
          ZIO.fromFuture(_ => invocation.result).map(result => assertTrue(exposedBeforeCompletion, result == Right(())))
        case Left(error) => ZIO.succeed(assertNever(s"expected started invocation, got $error"))
      }
    },
    test("preparation failures do not invoke the ambient transport") {
      val transport = new FakeTransport(Right(success()))
      val backend   = new AmbientToolCallBackend(transport)
      val call      = prepared(
        NoDeclaredErrors,
        (_: Option[TypedSchemaValue]) => Right(()),
        Left(ToolInvokeError.InvalidInput("bad local input"))
      )

      ZIO.fromFuture(_ => backend.awaitNoStdout(call)).map {
        case Left(ToolError.Rpc(RpcError.Protocol(message))) =>
          assertTrue(message == "bad local input", transport.calls == 0)
        case other => assertNever(s"expected local protocol error, got $other")
      }
    },
    test("nested inherited values follow descriptor canonical order") {
      val descriptor   = Right(nestedDescriptor)
      val stringSchema = implicitly[IntoSchema[String]]
      val boolSchema   = implicitly[IntoSchema[Boolean]]
      val inherited    = List(
        CanonicalInputValue("verbose", Nil, boolSchema.graph, boolSchema.toValue(true))
      )
      val params = Right(List("config" -> stringSchema.toValue("cfg")))

      ToolCallPreparation.prepareInput(descriptor, List("child", "run"), inherited, params) match {
        case Right(TypedSchemaValue(graph, SchemaValue.RecordValue(values))) =>
          val fields = graph.root.body.asInstanceOf[golem.schema.SchemaTypeBody.RecordType].fields.map(_.name)
          assertTrue(
            fields == List("config", "verbose"),
            values == List(SchemaValue.OptionValue(Some(stringSchema.toValue("cfg"))), boolSchema.toValue(true))
          )
        case other => assertNever(s"expected canonical record, got $other")
      }
    }
  )
}
