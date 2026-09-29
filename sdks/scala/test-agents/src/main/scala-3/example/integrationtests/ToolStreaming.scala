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

package example.integrationtests

import golem.BaseAgent
import golem.runtime.annotations.*
import golem.tool.{ByteStreamFailure, ToolInputStream, ToolOutputStream}
import zio.blocks.async.*
import zio.blocks.schema.Schema
import zio.blocks.streams.{JvmType, Stream}
import zio.blocks.streams.io.Reader

import scala.concurrent.{ExecutionContext, Future, Promise}

@toolDefinition(name = "scala-streaming", version = "1.0.0")
trait ScalaStreamingTool {
  def stream(
    mode: String,
    stdin: ToolInputStream,
    stdout: ToolOutputStream
  ): Future[Long]
}

@toolImplementation()
final class ScalaStreamingToolImpl extends ScalaStreamingTool {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def stream(
    mode: String,
    stdin: ToolInputStream,
    stdout: ToolOutputStream
  ): Future[Long] = {
    def requireWrite(result: Either[?, Unit]): Future[Unit] = result match {
      case Right(_)    => Future.successful(())
      case Left(error) => Future.failed(new IllegalStateException(s"stream write failed: $error"))
    }

    val copied = stdin.stream
      .runFoldAsync(0L) { (bytesRead, byte) =>
        Async.fromFuture(stdout.write(Array(byte)).flatMap(requireWrite)).map(_ => bytesRead + 1L)
      }
      .toFuture
      .flatMap {
        case Right(bytesRead) => Future.successful(bytesRead)
        case Left(error)      => Future.failed(new IllegalStateException(s"stream input failed: $error"))
      }

    val marker =
      if (mode == "marker-echo") stdout.write("scala-marker:".getBytes("UTF-8")).flatMap(requireWrite)
      else Future.successful(())
    marker.flatMap(_ => copied)
  }
}

final case class ScalaStreamEvidence(output: String, bytesRead: Long)
object ScalaStreamEvidence {
  implicit val schema: Schema[ScalaStreamEvidence] = Schema.derived
}

@agentDefinition()
trait ScalaToolStreamingCaller extends BaseAgent {
  class Id(val name: String)
  def markerBeforeEof(payload: String): Future[ScalaStreamEvidence]
}

@agentImplementation()
final class ScalaToolStreamingCallerImpl(name: String) extends ScalaToolStreamingCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def markerBeforeEof(payload: String): Future[ScalaStreamEvidence] = {
    val release      = Promise[Unit]()
    val payloadBytes = payload.getBytes("UTF-8")
    val stdin        = new ToolInputStream {
      override val stream: Stream[ByteStreamFailure, Byte] = Stream
        .unfoldAsync(false) { sent =>
          Async.fromFuture(release.future).map(_ => if (sent) None else Some(payloadBytes -> true))
        }(using JvmType.Infer.boxed[Array[Byte]])
        .flatMap(Stream.fromArray)

      override def cancel(): Future[Unit] = Future.successful(())
    }

    ScalaStreamingToolClient().stream("marker-echo", stdin) match {
      case Left(error)       => Future.failed(new IllegalStateException(s"failed to start Scala streaming tool: $error"))
      case Right(invocation) =>
        invocation.stdout.stream.startAsync.toFuture.flatMap { reader =>
          readN(reader, "scala-marker:".length).flatMap {
            case marker if marker.sameElements("scala-marker:".getBytes("UTF-8")) =>
              release.success(())
              val output = readAll(reader, Vector(marker))
              invocation.result.zip(output).flatMap {
                case (Right(bytesRead), bytes) =>
                  Future.successful(ScalaStreamEvidence(new String(bytes, "UTF-8"), bytesRead))
                case (Left(error), _) => Future.failed(new IllegalStateException(s"Scala tool failed: $error"))
              }
            case other =>
              Future.failed(new IllegalStateException(s"expected live Scala marker before stdin EOF, got $other"))
          }
        }
    }
  }

  private object End

  private def readN(reader: Reader.AsyncReader[Byte], count: Int): Future[Array[Byte]] = {
    def loop(bytes: Vector[Byte]): Future[Array[Byte]] =
      if (bytes.length == count) Future.successful(bytes.toArray)
      else
        reader.read[Any](End).toFuture.flatMap {
          case byte: Byte => loop(bytes :+ byte)
          case _          => Future.failed(new IllegalStateException("Scala stdout ended before the marker"))
        }
    loop(Vector.empty)
  }

  private def readAll(reader: Reader.AsyncReader[Byte], chunks: Vector[Array[Byte]]): Future[Array[Byte]] =
    reader.read[Any](End).toFuture.flatMap {
      case byte: Byte => readAll(reader, chunks :+ Array(byte))
      case _          => Future.successful(chunks.flatten.toArray)
    }
}
