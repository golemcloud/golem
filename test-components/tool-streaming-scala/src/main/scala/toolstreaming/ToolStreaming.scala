package toolstreaming

import golem.BaseAgent
import golem.runtime.annotations.*
import golem.runtime.tool.client.ToolRpcClient
import golem.schema.IntoSchema
import golem.tool.{ByteStreamFailure, ToolInputStream, ToolInvokeError, ToolOutputStream, ToolRpcFailure}
import zio.blocks.schema.Schema

import scala.concurrent.{ExecutionContext, Future, Promise}

@toolDefinition(name = "scala-streaming", version = "1.0.0")
trait ScalaStreamingTool {
  def stream(
      mode: String,
      stdin: ToolInputStream,
      stdout: ToolOutputStream
  ): Future[Long]

  def output(mode: String, stdout: ToolOutputStream): Future[String]
  def outputUnit(stdout: ToolOutputStream): Future[Unit]
  def plain(): String
}

@toolImplementation()
final class ScalaStreamingToolImpl extends ScalaStreamingTool {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  private def requireWrite(result: Either[?, Unit]): Future[Unit] = result match {
    case Right(_)    => Future.successful(())
    case Left(error) => Future.failed(new IllegalStateException(s"stream write failed: $error"))
  }

  override def outputUnit(stdout: ToolOutputStream): Future[Unit] =
    stdout.write(Array[Byte](0, 127, -128, -1)).flatMap(requireWrite)

  override def output(mode: String, stdout: ToolOutputStream): Future[String] =
    outputUnit(stdout).flatMap { _ =>
      val terminal = mode match {
        case "finish" => stdout.finish()
        case "fail"   => stdout.fail(ByteStreamFailure.Failed("partial-output"))
        case other    => throw new IllegalArgumentException(s"unknown output mode: $other")
      }
      terminal.flatMap(requireWrite).map(_ => "done")
    }

  override def plain(): String = "plain"

  override def stream(
      mode: String,
      stdin: ToolInputStream,
      stdout: ToolOutputStream
  ): Future[Long] = {
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

final case class ScalaCleanupEvidence(error: String, stdinCancelled: Boolean, stdoutTerminal: String)
object ScalaCleanupEvidence {
  implicit val schema: Schema[ScalaCleanupEvidence] = Schema.derived
}

final case class ScalaOutputEvidence(bytes: List[Int], terminal: String, result: String)
object ScalaOutputEvidence {
  implicit val schema: Schema[ScalaOutputEvidence] = Schema.derived
}

@agentDefinition()
trait ScalaToolStreamingCaller extends BaseAgent {
  class Id(val name: String)
  def markerBeforeEof(payload: String): Future[ScalaStreamEvidence]
  def invalidCommandPathCleanup(): Future[ScalaCleanupEvidence]
  def outputEvidence(mode: String): Future[ScalaOutputEvidence]
}

@agentImplementation()
final class ScalaToolStreamingCallerImpl(name: String) extends ScalaToolStreamingCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def outputEvidence(mode: String): Future[ScalaOutputEvidence] = {
    val client = ScalaStreamingToolClient()
    if (mode == "plain") {
      client.plain().map {
        case Right(result) => ScalaOutputEvidence(Nil, "none", result)
        case Left(error) => throw new IllegalStateException(s"plain tool failed: $error")
      }
    } else {
      val started =
        if (mode == "unit") client.outputUnit().map(invocation =>
          (invocation.stdout, invocation.result.map(_.map(_ => "unit"))))
        else client.output(mode).map(invocation => (invocation.stdout, invocation.result))

      def drain(stream: ToolInputStream, bytes: List[Int]): Future[(List[Int], String)] =
        stream.read().flatMap {
          case Right(Some(chunk)) => drain(stream, bytes ++ chunk.map(_ & 0xff).toList)
          case Right(None) => Future.successful((bytes, "finished"))
          case Left(ByteStreamFailure.Failed(message)) => Future.successful((bytes, s"failed:$message"))
          case Left(error) => Future.failed(new IllegalStateException(s"unexpected stdout failure: $error"))
        }

      started match {
        case Left(error) => Future.failed(new IllegalStateException(s"failed to start output tool: $error"))
        case Right((stdout, result)) =>
          result.zip(drain(stdout, Nil)).map {
            case (Right(value), (bytes, terminal)) => ScalaOutputEvidence(bytes, terminal, value)
            case (Left(error), _) => throw new IllegalStateException(s"output tool failed: $error")
          }
      }
    }
  }

  override def markerBeforeEof(payload: String): Future[ScalaStreamEvidence] = {
    val release = Promise[Unit]()
    val payloadBytes = payload.getBytes("UTF-8")
    val stdin = new ToolInputStream {
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
      case Left(error) => Future.failed(new IllegalStateException(s"failed to start Scala streaming tool: $error"))
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

  override def invalidCommandPathCleanup(): Future[ScalaCleanupEvidence] = {
    val sourceCompletion = Promise[Either[ByteStreamFailure, Option[Array[Byte]]]]()
    val sourceCancelled = Promise[Unit]()
    val stdin = new ToolInputStream {
      override def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
        sourceCompletion.future

      override def cancel(): Future[Unit] = {
        sourceCancelled.trySuccess(())
        sourceCompletion.trySuccess(Right(None))
        Future.successful(())
      }
    }

    ToolRpcClient
      .transport("scala-streaming")
      .start(
        List("missing"),
        IntoSchema[String].toTyped("ignored"),
        Some(stdin),
        stdout = true
      ) match {
      case Left(error) => Future.failed(new IllegalStateException(s"failed to start invalid-path invocation: $error"))
      case Right(invocation) =>
        invocation.stdout match {
          case None => Future.failed(new IllegalStateException("invalid-path invocation did not provide stdout"))
          case Some(stdout) =>
            for {
              result <- invocation.result
              terminal <- stdout.read()
              _ <- sourceCancelled.future
            } yield {
              val error = result match {
                case Left(ToolRpcFailure.RemoteToolError(ToolInvokeError.InvalidCommandPath(path))) =>
                  s"invalid-command-path:${path.mkString("/")}"
                case other => s"unexpected:$other"
              }
              val stdoutTerminal = terminal match {
                case Right(None)                               => "closed"
                case Right(Some(bytes))                        => s"unexpected-chunk:${bytes.length}"
                case Left(ByteStreamFailure.Cancelled)         => "cancelled"
                case Left(ByteStreamFailure.Abandoned)         => "abandoned"
                case Left(ByteStreamFailure.ResourceExhausted) => "resource-exhausted"
                case Left(ByteStreamFailure.Failed(_))         => "failed"
              }
              ScalaCleanupEvidence(
                error,
                stdinCancelled = true,
                stdoutTerminal
              )
            }
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
