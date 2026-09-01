package toolstreaming

import golem.BaseAgent
import golem.runtime.annotations.*
import golem.tool.{ByteStreamFailure, ToolInputStream, ToolOutputStream}
import zio.blocks.schema.Schema

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

    def read(bytesRead: Long): Future[Long] =
      stdin.read().flatMap {
        case Right(Some(bytes)) =>
          stdout.write(bytes).flatMap(requireWrite).flatMap(_ => read(bytesRead + bytes.length))
        case Right(None) => Future.successful(bytesRead)
        case Left(error) => Future.failed(new IllegalStateException(s"stream input failed: $error"))
      }

    val marker =
      if (mode == "marker-echo") stdout.write("scala-marker:".getBytes("UTF-8")).flatMap(requireWrite)
      else Future.successful(())
    marker.flatMap(_ => read(0L))
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
      private var sent = false

      override def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
        release.future.map { _ =>
          if (sent) Right(None)
          else {
            sent = true
            Right(Some(payloadBytes))
          }
        }

      override def cancel(): Future[Unit] = Future.successful(())
    }

    ScalaStreamingToolClient().stream("marker-echo", stdin) match {
      case Left(error)       => Future.failed(new IllegalStateException(s"failed to start Scala streaming tool: $error"))
      case Right(invocation) =>
        invocation.stdout.read().flatMap {
          case Right(Some(marker)) if marker.sameElements("scala-marker:".getBytes("UTF-8")) =>
            release.success(())
            val output = readAll(invocation.stdout, Vector(marker))
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

  private def readAll(stream: ToolInputStream, chunks: Vector[Array[Byte]]): Future[Array[Byte]] =
    stream.read().flatMap {
      case Right(Some(chunk)) => readAll(stream, chunks :+ chunk)
      case Right(None)        => Future.successful(chunks.flatten.toArray)
      case Left(error)        => Future.failed(new IllegalStateException(s"Scala stdout failed: $error"))
    }
}
