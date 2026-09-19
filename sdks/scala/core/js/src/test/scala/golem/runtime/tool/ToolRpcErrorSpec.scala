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
import golem.FutureInterop
import golem.runtime.tool.host.ToolHostApi
import golem.runtime.tool.client.JsToolRpcTransport
import golem.schema.{IntoSchema, TypedSchemaValue}
import golem.schema.wire.SchemaWire
import golem.tool.{
  ByteStreamCloseCause,
  ByteStreamFailure,
  StreamWriteError,
  ToolInputStream,
  ToolInvokeError,
  ToolRpcFailure
}
import golem.tool.wire.{WitCustomToolError, WitToolError}
import zio.test._
import zio.ZIO

import scala.collection.mutable.ListBuffer
import scala.concurrent.{Future, Promise}
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * Verifies the `golem:tool/host@0.1.0` `rpc-error` decoding used by the typed
 * tool client transport: the string-carrying cases, the `remote-tool-error`
 * payload round trip, and the defensive fallback for foreign thrown values.
 */
object ToolRpcErrorSpec extends ZIOSpecDefault {

  private def variant(tag: String, value: js.Any): js.Any =
    js.Dynamic.literal("tag" -> tag, "val" -> value).asInstanceOf[js.Any]

  private def payload(text: String): TypedSchemaValue =
    implicitly[IntoSchema[String]].toTyped(text)

  def spec = suite("ToolRpcErrorSpec")(
    test("decodes the string-carrying error cases") {
      assertTrue(
        ToolHostApi.decodeRpcFailure(variant("protocol-error", "bad frame")) ==
          ToolRpcFailure.ProtocolError("bad frame"),
        ToolHostApi.decodeRpcFailure(variant("denied", "nope")) ==
          ToolRpcFailure.Denied("nope"),
        ToolHostApi.decodeRpcFailure(variant("not-found", "missing")) ==
          ToolRpcFailure.NotFound("missing"),
        ToolHostApi.decodeRpcFailure(variant("remote-internal-error", "boom")) ==
          ToolRpcFailure.RemoteInternalError("boom")
      )
    },
    test("decodes remote-tool-error preserving the custom-error payload") {
      val original = payload("bad flag")
      val jsError  = ToolWireInterop.toolErrorToJs(
        WitToolError.CustomError(WitCustomToolError("failure", SchemaWire.typedSchemaValueToWit(original)))
      )
      val decoded = ToolHostApi.decodeRpcFailure(variant("remote-tool-error", jsError))
      decoded match {
        case ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError(name, roundTripped)) =>
          assertTrue(name == "failure", roundTripped == original)
        case other =>
          assertNever(s"expected remote tool custom error, got: $other")
      }
    },
    test("decodes remote-tool-error framing cases") {
      val jsError = ToolWireInterop.toolErrorToJs(WitToolError.InvalidInput("bad wire input"))
      assertTrue(
        ToolHostApi.decodeRpcFailure(variant("remote-tool-error", jsError)) ==
          ToolRpcFailure.RemoteToolError(ToolInvokeError.InvalidInput("bad wire input"))
      )
    },
    test("falls back to a protocol error for an unrecognized thrown value") {
      assertTrue(
        ToolHostApi.decodeRpcFailure("not a js error") ==
          ToolRpcFailure.ProtocolError("not a js error"),
        ToolHostApi.decodeRpcFailure(variant("mystery", "x")) ==
          ToolRpcFailure.ProtocolError("unknown rpc error `mystery`")
      )
    },
    test("decodes stream write errors and nested close failures") {
      val failed = variant("failed", variant("failed", "source failed"))
      assertTrue(
        ToolHostApi
          .decodeStreamWriteError(js.Dynamic.literal("tag" -> "concurrent-operation"))
          .contains(StreamWriteError.ConcurrentOperation),
        ToolHostApi
          .decodeStreamWriteError(variant("closed", js.Dynamic.literal("tag" -> "finished")))
          .contains(StreamWriteError.Closed(ByteStreamCloseCause.Finished)),
        ToolHostApi
          .decodeStreamWriteError(variant("closed", failed))
          .contains(
            StreamWriteError.Closed(ByteStreamCloseCause.Failed(ByteStreamFailure.Failed("source failed")))
          ),
        ToolHostApi.decodeStreamWriteError(js.Dynamic.literal("tag" -> "unknown")).isEmpty
      )
    },
    test("maps a rejected host writer promise to its typed stream error") {
      val rejection = variant("closed", js.Dynamic.literal("tag" -> "consumer-cancelled"))
      val writer    = js.Dynamic
        .literal(
          "write" -> js.Any.fromFunction1((_: js.typedarray.Uint8Array) =>
            js.Promise.reject(rejection).asInstanceOf[js.Promise[Unit]]
          ),
          "finish" -> js.Any.fromFunction0(() => js.Promise.resolve[Unit](())),
          "fail"   -> js.Any.fromFunction1((_: js.Any) => js.Promise.resolve[Unit](()))
        )
        .asInstanceOf[ToolHostApi.RawToolStdoutWriter]
      ZIO.fromFuture(_ => new JsToolOutputStream(writer).write(Array[Byte](1))).map { result =>
        assertTrue(result == Left(StreamWriteError.Closed(ByteStreamCloseCause.ConsumerCancelled)))
      }
    },
    test("skips an empty provider stdout write before calling the host") {
      var writes = 0
      val writer = js.Dynamic
        .literal(
          "write" -> js.Any.fromFunction1 { (_: js.typedarray.Uint8Array) =>
            writes += 1
            js.Promise.resolve[Unit](())
          },
          "finish" -> js.Any.fromFunction0(() => js.Promise.resolve[Unit](())),
          "fail"   -> js.Any.fromFunction1((_: js.Any) => js.Promise.resolve[Unit](()))
        )
        .asInstanceOf[ToolHostApi.RawToolStdoutWriter]
      ZIO.fromFuture(_ => new JsToolOutputStream(writer).write(Array.emptyByteArray)).map { result =>
        assertTrue(result == Right(()), writes == 0)
      }
    },
    test("retries a terminal rejected because another operation is outstanding") {
      var finishes   = 0
      val concurrent = js.Dynamic.literal("tag" -> "concurrent-operation")
      val writer     = js.Dynamic
        .literal(
          "write"  -> js.Any.fromFunction1((_: js.typedarray.Uint8Array) => js.Promise.resolve[Unit](())),
          "finish" -> js.Any.fromFunction0 { () =>
            finishes += 1
            if (finishes == 1)
              js.Promise.reject(concurrent).asInstanceOf[js.Promise[Unit]]
            else js.Promise.resolve[Unit](())
          },
          "fail" -> js.Any.fromFunction1((_: js.Any) => js.Promise.resolve[Unit](()))
        )
        .asInstanceOf[ToolHostApi.RawToolStdoutWriter]
      val stream = new JsToolOutputStream(writer)

      for {
        first  <- ZIO.fromFuture(_ => stream.finish())
        second <- ZIO.fromFuture(_ => stream.finish())
      } yield assertTrue(
        first == Left(StreamWriteError.ConcurrentOperation),
        second == Right(()),
        finishes == 2
      )
    },
    test("provider finalization waits for writes and terminal before disposing exactly once") {
      val writeDone     = Promise[Unit]()
      val finishStarted = Promise[Unit]()
      val finishDone    = Promise[Unit]()
      var bytes         = List.empty[Int]
      var disposals     = 0
      val writer        = js.Dynamic.literal(
        "write" -> js.Any.fromFunction1 { (chunk: js.typedarray.Uint8Array) =>
          bytes = chunk.toArray.map(_.toInt).toList
          FutureInterop.toPromise(writeDone.future)
        },
        "finish" -> js.Any.fromFunction0 { () =>
          finishStarted.success(())
          FutureInterop.toPromise(finishDone.future)
        }
      )
      js.Dynamic.global.Reflect
        .set(writer, js.Dynamic.global.Symbol.selectDynamic("dispose"), js.Any.fromFunction0(() => disposals += 1))
      val stream  = new JsToolOutputStream(writer.asInstanceOf[ToolHostApi.RawToolStdoutWriter])
      val write   = stream.write(Array[Byte](0, 127, -128, -1))
      val closed  = stream.close()
      val between = write.flatMap(_ => stream.write(Array[Byte](42)))(scala.concurrent.ExecutionContext.parasitic)
      for {
        concurrent <- ZIO.fromFuture(_ => stream.finish())
        before      = (closed.isCompleted, finishStarted.isCompleted, disposals)
        _          <- ZIO.succeed(writeDone.success(()))
        written    <- ZIO.fromFuture(_ => write)
        _          <- ZIO.fromFuture(_ => finishStarted.future)
        during      = (closed.isCompleted, disposals)
        _          <- ZIO.succeed(finishDone.success(()))
        _          <- ZIO.fromFuture(_ => closed)
        _          <- ZIO.fromFuture(_ => stream.close())
        late       <- ZIO.fromFuture(_ => stream.write(Array.emptyByteArray))
        raced      <- ZIO.fromFuture(_ => between)
      } yield assertTrue(
        before == (false, false, 0),
        during == (false, 0),
        bytes == List(0, 127, 128, 255),
        concurrent == Left(StreamWriteError.ConcurrentOperation),
        raced == Left(StreamWriteError.ConcurrentOperation),
        written == Right(()),
        disposals == 1,
        late == Left(StreamWriteError.Closed(ByteStreamCloseCause.Finished))
      )
    },
    test("explicit failure remains immutable through competing terminals and invocation cleanup") {
      var fails     = 0
      var finishes  = 0
      var disposals = 0
      var failure   = ""
      val writer    = js.Dynamic.literal(
        "fail" -> js.Any.fromFunction1 { (reason: js.Dynamic) =>
          fails += 1
          failure = reason.tag.asInstanceOf[String]
          js.Promise.resolve[Unit](())
        },
        "finish" -> js.Any.fromFunction0 { () => finishes += 1; js.Promise.resolve[Unit](()) }
      )
      js.Dynamic.global.Reflect
        .set(writer, js.Dynamic.global.Symbol.selectDynamic("dispose"), js.Any.fromFunction0(() => disposals += 1))
      val stream = new JsToolOutputStream(writer.asInstanceOf[ToolHostApi.RawToolStdoutWriter])
      for {
        failed    <- ZIO.fromFuture(_ => stream.fail(ByteStreamFailure.ResourceExhausted))
        repeated  <- ZIO.fromFuture(_ => stream.fail(ByteStreamFailure.ResourceExhausted))
        competing <- ZIO.fromFuture(_ => stream.finish())
        _         <- ZIO.fromFuture(_ => stream.close())
      } yield assertTrue(
        failed == Right(()),
        repeated == Right(()),
        competing == Left(StreamWriteError.Closed(ByteStreamCloseCause.Failed(ByteStreamFailure.ResourceExhausted))),
        fails == 1,
        finishes == 0,
        disposals == 1,
        failure == "resource-exhausted"
      )
    },
    test("finalization releases writers after synchronous and asynchronous failures or consumer closure") {
      ZIO
        .foreach(List("sync", "async", "closed")) { mode =>
          var disposals = 0
          val writer    = js.Dynamic.literal(
            "finish" -> js.Any.fromFunction0 { () =>
              mode match {
                case "sync"  => throw new RuntimeException("finish failed")
                case "async" => js.Promise.reject(new RuntimeException("finish failed")).asInstanceOf[js.Promise[Unit]]
                case _       =>
                  js.Promise
                    .reject(variant("closed", js.Dynamic.literal("tag" -> "consumer-cancelled")))
                    .asInstanceOf[js.Promise[Unit]]
              }
            }
          )
          js.Dynamic.global.Reflect
            .set(writer, js.Dynamic.global.Symbol.selectDynamic("dispose"), js.Any.fromFunction0(() => disposals += 1))
          val stream = new JsToolOutputStream(writer.asInstanceOf[ToolHostApi.RawToolStdoutWriter])
          for {
            closed <- ZIO.fromFuture(_ => stream.close()).either
            again  <- ZIO.fromFuture(_ => stream.close()).either
            late   <- ZIO.fromFuture(_ => stream.write(Array[Byte](1)))
          } yield assertTrue(
            closed.isRight == (mode == "closed"),
            again.isRight == closed.isRight,
            disposals == 1,
            late == Left(
              StreamWriteError.Closed(
                if (mode == "closed") ByteStreamCloseCause.ConsumerCancelled
                else ByteStreamCloseCause.Failed(ByteStreamFailure.Abandoned)
              )
            )
          )
        }
        .map(_.reduce(_ && _))
    },
    test("skips empty caller stdin chunks before writing to the host") {
      val writes   = ListBuffer.empty[Array[Byte]]
      val finished = Promise[Unit]()
      val source   = new ToolInputStream {
        private var reads = 0

        override def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] = {
          reads += 1
          Future.successful(
            reads match {
              case 1 => Right(Some(Array.emptyByteArray))
              case 2 => Right(Some(Array[Byte](1, 2)))
              case _ => Right(None)
            }
          )
        }
      }
      val writer = js.Dynamic
        .literal(
          "write" -> js.Any.fromFunction1 { (bytes: js.typedarray.Uint8Array) =>
            writes += bytes.toArray.map(_.toByte)
            js.Promise.resolve[Unit](())
          },
          "finish" -> js.Any.fromFunction0 { () =>
            finished.success(())
            js.Promise.resolve[Unit](())
          },
          "fail" -> js.Any.fromFunction1((_: js.Any) => js.Promise.resolve[Unit](()))
        )
        .asInstanceOf[ToolHostApi.RawToolStdinWriter]
      val neverClosed = Promise[js.Any]()
      val closed      = js.Dynamic
        .literal(
          "wait" -> js.Any.fromFunction0(() => FutureInterop.toPromise(neverClosed.future))
        )
        .asInstanceOf[ToolHostApi.RawToolStdinClosed]

      new JsToolRpcTransport(null.asInstanceOf[ToolHostApi.RawToolRpc]).pump(source, writer, closed)

      ZIO.fromFuture(_ => finished.future).map { _ =>
        assertTrue(writes.toList.map(_.toList) == List(List[Byte](1, 2)))
      }
    }
  )
}
