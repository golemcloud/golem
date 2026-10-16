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

import golem.schema.{FromSchema, IntoSchema, TypedSchemaValue}
import zio.ZIO
import zio.test._

import scala.concurrent.{ExecutionContext, Future, Promise}
import zio.blocks.async.*
import zio.blocks.streams.{JvmType, Stream}

import java.util.concurrent.CancellationException

/**
 * The Scala port of the Rust SDK's `tool_client.rs` unit tests: custom-error
 * payload decoding, custom-error decode through `invokeAndAwait`, and framing
 * error mapping, all against fake transports.
 */
object ToolClientSpec extends ZIOSpecDefault {

  private sealed trait CliError                   extends Product with Serializable
  private final case class Usage(message: String) extends CliError

  private def decodeCliError(error: NamedToolError): Either[String, CliError] =
    if (error.name != "usage") Left("unknown error")
    else
      implicitly[FromSchema[String]]
        .fromValue(error.payload.value)
        .map(Usage(_): CliError)
        .left
        .map(e => s"failed to decode remote tool error: ${e.message}")

  private val unitInput: TypedSchemaValue = ToolErrorSupport.unitPayload

  private final class ChunkStream(initial: List[Either[ByteStreamFailure, Option[Array[Byte]]]])
      extends ToolInputStream {
    override val stream: Stream[ByteStreamFailure, Byte] = initial.foldRight(Stream.empty) {
      case (Left(error), _)           => Stream.fail(error)
      case (Right(None), _)           => Stream.empty
      case (Right(Some(chunk)), tail) => Stream.fromArray(chunk) ++ tail
    }
    override def cancel(): Future[Unit] = Future.successful(())
  }

  private final class FailedReadStream(failure: Throwable) extends ToolInputStream {
    override val stream: Stream[ByteStreamFailure, Byte] = Stream.unfoldAsync(()) { _ =>
      Async.fromFuture[Option[(Byte, Unit)]](Future.failed(failure))
    }(using JvmType.Infer.byte)
    override def cancel(): Future[Unit] = Future.successful(())
  }

  private final class ThrowingStream(failure: Throwable) extends ToolInputStream {
    override def stream: Stream[ByteStreamFailure, Byte] = throw failure
    override def cancel(): Future[Unit]                  = Future.successful(())
  }

  private def stringPayload(text: String): TypedSchemaValue =
    implicitly[IntoSchema[String]].toTyped(text)

  private final class FakeToolRpc extends ToolRpcTransport {
    def start(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream],
      stdout: Boolean,
      stderr: Boolean
    ): Either[ToolRpcFailure, ToolRpcStarted] =
      Right(
        ToolRpcStarted(
          None,
          None,
          Future.successful(
            Left(ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError("usage", stringPayload("bad flag"))))
          ),
          () => ()
        )
      )
  }

  private sealed trait FakeFailure
  private object FakeFailure {
    case object Denied             extends FakeFailure
    case object RemoteInvalidInput extends FakeFailure
  }

  private final class FailingToolRpc(failure: FakeFailure) extends ToolRpcTransport {
    def start(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream],
      stdout: Boolean,
      stderr: Boolean
    ): Either[ToolRpcFailure, ToolRpcStarted] =
      Right(
        ToolRpcStarted(
          None,
          None,
          Future.successful(Left(failure match {
            case FakeFailure.Denied             => ToolRpcFailure.Denied("no access")
            case FakeFailure.RemoteInvalidInput =>
              ToolRpcFailure.RemoteToolError(ToolInvokeError.InvalidInput("bad wire input"))
          })),
          () => ()
        )
      )
  }

  def spec: Spec[Any, Any] = suite("ToolClientSpec")(
    test("custom_tool_error_payload_decodes_to_declared_error_variant") {
      val decoded = ToolClientRuntime.mapRemoteToolError(
        ToolInvokeError.UnknownToolError("usage", stringPayload("bad flag")),
        decodeCliError
      )
      assertTrue(decoded == ToolError.Tool(Usage("bad flag")))
    },
    test("unknown and malformed known custom errors retain their names and payloads") {
      val unknownPayload   = stringPayload("future case")
      val malformedPayload = implicitly[IntoSchema[Int]].toTyped(42)
      assertTrue(
        ToolClientRuntime.mapRemoteToolError(
          ToolInvokeError.UnknownToolError("future-error", unknownPayload),
          decodeCliError
        ) == ToolError.UnknownToolError("future-error", unknownPayload),
        ToolClientRuntime.mapRemoteToolError(
          ToolInvokeError.UnknownToolError("usage", malformedPayload),
          decodeCliError
        ) == ToolError.UnknownToolError("usage", malformedPayload)
      )
    },
    test("invoke_and_await_decoding_error_decodes_custom_tool_error_payload") {
      ZIO
        .fromFuture(_ => ToolClientRuntime.invokeAndAwait(new FakeToolRpc, Nil, unitInput, None, decodeCliError))
        .map {
          case Left(ToolError.Tool(Usage(message))) => assertTrue(message == "bad flag")
          case other                                => assertNever(s"expected declared tool error, got: $other")
        }
    },
    test("started invocation collect drains chunks in order concurrently with the result") {
      val stream = new ChunkStream(
        List(
          Right(Some(Array[Byte](1, 2))),
          Right(Some(Array[Byte](3))),
          Right(None)
        )
      )
      val invocation = ToolInvocation[Nothing, String](Some(stream), None, Future.successful(Right("done")), () => ())
      ZIO.fromFuture(ec => invocation.collect()(ec)).map { result =>
        assertTrue(
          result.result == Right("done"),
          result.stdout.exists(_.exists(_.sameElements(Array[Byte](1, 2, 3)))),
          result.stderr == Right(None)
        )
      }
    },
    test("started invocation collect retains a failed result future after both outputs settle") {
      val eof    = Promise[Unit]()
      val stream = new ToolInputStream {
        override val stream: Stream[ByteStreamFailure, Byte] = Stream
          .unfoldAsync(false) { emitted =>
            if (emitted) Async.succeed(None)
            else Async.fromFuture(eof.future).map(_ => None)
          }(using JvmType.Infer.byte)
        override def cancel(): Future[Unit] = Future.successful(())
      }
      val failure    = new RuntimeException("observer failed")
      val invocation = ToolInvocation[Nothing, Unit](
        Some(stream),
        Some(new ChunkStream(List(Right(Some(Array[Byte](9))), Right(None)))),
        Future.failed(failure),
        () => ()
      )
      val collected = invocation.collect()(ExecutionContext.global)
      for {
        _      <- ZIO.yieldNow
        before  = !collected.isCompleted
        _       = eof.success(())
        result <- ZIO.fromFuture(_ => collected)
      } yield assertTrue(
        before,
        result.result == Left(ToolError.Rpc(RpcError.Protocol("observer failed"))),
        result.stdout.exists(_.exists(_.isEmpty)),
        result.stderr.exists(_.exists(_.sameElements(Array[Byte](9))))
      )
    },
    test("started invocation collect retains a failed read future without hiding its sibling output") {
      val failure    = new RuntimeException("stdout read failed")
      val invocation = ToolInvocation[Nothing, String](
        Some(new FailedReadStream(failure)),
        Some(new ChunkStream(List(Right(Some(Array[Byte](7, 8))), Right(None)))),
        Future.successful(Right("done")),
        () => ()
      )
      ZIO.fromFuture(ec => invocation.collect()(ec)).map { result =>
        assertTrue(
          result.result == Right("done"),
          result.stdout == Left(ByteStreamFailure.Failed("stdout read failed")),
          result.stderr.exists(_.exists(_.sameElements(Array[Byte](7, 8))))
        )
      }
    },
    test("started invocation collect retains a synchronous stream failure in its channel") {
      val failure    = new RuntimeException("stderr stream failed")
      val invocation = ToolInvocation[Nothing, String](
        Some(new ChunkStream(List(Right(Some(Array[Byte](3, 4))), Right(None)))),
        Some(new ThrowingStream(failure)),
        Future.successful(Right("done")),
        () => ()
      )
      ZIO.fromFuture(ec => invocation.collect()(ec)).map { result =>
        assertTrue(
          result.result == Right("done"),
          result.stdout.exists(_.exists(_.sameElements(Array[Byte](3, 4)))),
          result.stderr == Left(ByteStreamFailure.Failed("stderr stream failed"))
        )
      }
    },
    test("started invocation collect does not convert cancellation into a collected result") {
      val cancellation = new CancellationException("cancelled")
      val invocation   = ToolInvocation[Nothing, Unit](None, None, Future.failed(cancellation), () => ())
      ZIO.fromFuture(ec => invocation.collect()(ec).failed).map(error => assertTrue(error eq cancellation))
    },
    test("started invocation collect returns a failed future for synchronous stream cancellation") {
      val cancellation = new CancellationException("cancelled while opening stream")
      val invocation   = ToolInvocation[Nothing, Unit](
        Some(new ThrowingStream(cancellation)),
        None,
        Future.successful(Right(())),
        () => ()
      )
      val attempted = scala.util.Try(invocation.collect()(ExecutionContext.global))
      assertTrue(
        attempted.isSuccess,
        attempted.toOption.exists(future => future.value.exists(_.failed.toOption.contains(cancellation)))
      )
    },
    test("started invocation collect does not convert wrapped cancellation") {
      val cancellation = new CancellationException("cancelled")
      val boxed        = new RuntimeException("wrapped", cancellation)
      val invocation   = ToolInvocation[Nothing, Unit](None, None, Future.failed(boxed), () => ())
      ZIO.fromFuture(ec => invocation.collect()(ec).failed).map(error => assertTrue(error eq boxed))
    },
    test("started invocation collect handles cyclic non-fatal cause chains") {
      val first  = new RuntimeException("first")
      val second = new RuntimeException("second")
      first.initCause(second)
      second.initCause(first)
      val invocation = ToolInvocation[Nothing, Unit](None, None, Future.failed(first), () => ())
      ZIO.fromFuture(ec => invocation.collect()(ec)).map(result => assertTrue(result.result.isLeft))
    } @@ TestAspect.timeout(zio.Duration.fromSeconds(2)),
    test("started invocation collect does not convert boxed fatal or interrupted failures") {
      val fatal                 = new LinkageError("fatal result")
      val interrupted           = new InterruptedException("interrupted read")
      val fatalInvocation       = ToolInvocation[Nothing, Unit](None, None, Future.failed(fatal), () => ())
      val interruptedInvocation = ToolInvocation[Nothing, Unit](
        Some(new FailedReadStream(interrupted)),
        None,
        Future.successful(Right(())),
        () => ()
      )
      for {
        fatalError       <- ZIO.fromFuture(ec => fatalInvocation.collect()(ec).failed)
        interruptedError <- ZIO.fromFuture(ec => interruptedInvocation.collect()(ec).failed)
      } yield assertTrue(
        fatalError.getCause eq fatal,
        interruptedError.getCause eq interrupted
      )
    },
    test("started invocation collect preserves a declared error when stdout also fails") {
      val declared   = Usage("bad flag")
      val stream     = new ChunkStream(List(Left(ByteStreamFailure.ResourceExhausted)))
      val invocation = ToolInvocation[CliError, Unit](
        Some(stream),
        None,
        Future.successful(Left(ToolError.Tool(declared))),
        () => ()
      )
      ZIO.fromFuture(ec => invocation.collect()(ec)).map { result =>
        assertTrue(
          result.result == Left(ToolError.Tool(declared)),
          result.stdout == Left(ByteStreamFailure.ResourceExhausted),
          result.stderr == Right(None)
        )
      }
    },
    test("started invocation cancellation is explicit and observer drop does not invoke it") {
      var cancelled  = false
      val invocation = ToolInvocation[Nothing, Unit](
        Some(new ChunkStream(List(Right(None)))),
        None,
        Future.successful(Right(())),
        () => cancelled = true
      )
      val dropped = invocation
      val before  = !cancelled && dropped.result.isCompleted
      invocation.cancel()
      assertTrue(before, cancelled)
    },
    test("invoke_and_await_distinguishes_rpc_and_remote_tool_errors") {
      for {
        denied <- ZIO.fromFuture(_ =>
                    ToolClientRuntime.invokeAndAwaitPayloadError[String](
                      new FailingToolRpc(FakeFailure.Denied),
                      Nil,
                      unitInput,
                      None
                    )
                  )
        remote <- ZIO.fromFuture(_ =>
                    ToolClientRuntime.invokeAndAwaitPayloadError[String](
                      new FailingToolRpc(FakeFailure.RemoteInvalidInput),
                      Nil,
                      unitInput,
                      None
                    )
                  )
      } yield {
        val deniedOk = denied match {
          case Left(ToolError.Rpc(RpcError.Denied(message))) => message == "no access"
          case _                                             => false
        }
        val remoteOk = remote match {
          case Left(ToolError.RemoteTool(ToolInvokeError.InvalidInput(message))) =>
            message == "bad wire input"
          case _ => false
        }
        assertTrue(deniedOk, remoteOk)
      }
    },
    test("all structural remote tool errors retain their variants") {
      val errors: List[ToolInvokeError[TypedSchemaValue]] = List(
        ToolInvokeError.InvalidToolName("bad name"),
        ToolInvokeError.InvalidCommandPath(List("bad")),
        ToolInvokeError.InvalidInput("input"),
        ToolInvokeError.ConstraintViolation("constraint"),
        ToolInvokeError.InvalidResult("result")
      )
      assertTrue(
        errors.forall(error =>
          ToolClientRuntime.mapRemoteToolError(error, decodeCliError) == ToolError.RemoteTool(error)
        )
      )
    },
    test("infallible pending results retain structural remote tool errors") {
      ZIO
        .fromFuture(_ =>
          ToolClientRuntime
            .invokeAndAwaitInfallible(new FailingToolRpc(FakeFailure.RemoteInvalidInput), Nil, unitInput, None)
        )
        .map(result => assertTrue(result == Left(ToolError.RemoteTool(ToolInvokeError.InvalidInput("bad wire input")))))
    }
  )
}
