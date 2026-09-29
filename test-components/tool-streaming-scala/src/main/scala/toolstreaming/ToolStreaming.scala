package toolstreaming

import golem.BaseAgent
import golem.runtime.annotations.*
import golem.runtime.tool.client.ToolRpcClient
import golem.schema.IntoSchema
import golem.tool.{ByteStreamFailure, ToolError, ToolInputStream, ToolInvokeError, ToolOutputStream, ToolRpcFailure}
import zio.blocks.async.*
import zio.blocks.schema.Schema
import zio.blocks.streams.{JvmType, Stream}
import zio.blocks.streams.io.Reader
import zio.blocks.streams.internal.StreamError

import scala.concurrent.{ExecutionContext, Future, Promise}

enum ScalaStreamingError {
  @error(kind = "runtime", exitCode = 1)
  case Expected(message: String)
}

@toolDefinition(name = "scala-streaming", version = "1.0.0")
trait ScalaStreamingTool {
  def stream(
      mode: String,
      stdin: ToolInputStream,
      stdout: ToolOutputStream
  ): Future[Long]

  def output(mode: String, stdout: ToolOutputStream): Future[String]
  def outputUnit(stdout: ToolOutputStream): Future[Unit]
  def declaredOutput(stdout: ToolOutputStream): Future[Either[ScalaStreamingError, String]]
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

  override def declaredOutput(stdout: ToolOutputStream): Future[Either[ScalaStreamingError, String]] =
    stdout
      .write("scala-declared:".getBytes("UTF-8"))
      .flatMap(requireWrite)
      .map(_ => Left(ScalaStreamingError.Expected("expected")))

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
    def copied =
      stdin.stream
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
  def declaredErrorCompletion(): Future[ScalaOutputEvidence]
}

@agentImplementation()
final class ScalaToolStreamingCallerImpl(name: String) extends ScalaToolStreamingCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def declaredErrorCompletion(): Future[ScalaOutputEvidence] =
    ScalaStreamingToolClient().declaredOutput() match {
      case Left(error) => Future.failed(new IllegalStateException(s"failed to start declared-output tool: $error"))
      case Right(invocation) =>
        invocation.result.zip(drain(invocation.stdout)).map {
          case (Left(ToolError.Tool(ScalaStreamingError.Expected(message))), (bytes, terminal)) =>
            ScalaOutputEvidence(bytes, terminal, s"declared:$message")
          case (other, (bytes, terminal)) =>
            ScalaOutputEvidence(bytes, terminal, s"unexpected:$other")
        }
    }

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

      started match {
        case Left(error) => Future.failed(new IllegalStateException(s"failed to start output tool: $error"))
        case Right((stdout, result)) =>
          result.zip(drain(stdout)).map {
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
      override val stream: Stream[ByteStreamFailure, Byte] = Stream
        .unfoldAsync(false) { sent =>
          Async.fromFuture(release.future).map(_ => if (sent) None else Some(payloadBytes -> true))
        }(using JvmType.Infer.boxed[Array[Byte]])
        .flatMap(Stream.fromArray)

      override def cancel(): Future[Unit] = Future.successful(())
    }

    ScalaStreamingToolClient().stream("marker-echo", stdin) match {
      case Left(error) => Future.failed(new IllegalStateException(s"failed to start Scala streaming tool: $error"))
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
              Future.failed(new IllegalStateException(s"expected live Scala marker before stdin EOF, got ${other.toSeq}"))
          }
        }
    }
  }

  override def invalidCommandPathCleanup(): Future[ScalaCleanupEvidence] = {
    val sourceCompletion = Promise[Unit]()
    val sourceCancelled = Promise[Unit]()
    val stdin = new ToolInputStream {
      override val stream: Stream[ByteStreamFailure, Byte] = Stream
        .unfoldAsync(false) { emitted =>
          if (emitted) Async.succeed(None)
          else Async.fromFuture(sourceCompletion.future).map(_ => None)
        }(using JvmType.Infer.byte)

      override def cancel(): Future[Unit] = {
        sourceCancelled.trySuccess(())
        sourceCompletion.trySuccess(())
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
              terminal <- readTerminal(stdout)
              _ <- sourceCancelled.future
            } yield {
              val error = result match {
                case Left(ToolRpcFailure.RemoteToolError(ToolInvokeError.InvalidCommandPath(path))) =>
                  s"invalid-command-path:${path.mkString("/")}"
                case other => s"unexpected:$other"
              }
              ScalaCleanupEvidence(
                error,
                stdinCancelled = true,
                terminal
              )
            }
        }
    }
  }

  private object End

  private def drain(stream: ToolInputStream): Future[(List[Int], String)] =
    stream.stream.startAsync.toFuture.flatMap(reader => drain(reader, Nil))

  private def drain(reader: Reader.AsyncReader[Byte], bytes: List[Int]): Future[(List[Int], String)] =
    reader.read[Any](End).toFuture
      .flatMap {
        case byte: Byte => drain(reader, bytes :+ (byte & 0xff))
        case _          => Future.successful((bytes, "finished"))
      }
      .recover {
        case error: StreamError =>
          error.value.asInstanceOf[ByteStreamFailure] match {
            case ByteStreamFailure.Failed(message) => (bytes, s"failed:$message")
            case failure                           => throw new IllegalStateException(s"unexpected stdout failure: $failure")
          }
        case error => throw new IllegalStateException(s"unexpected stdout failure: $error")
      }

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

  private def readTerminal(stdout: ToolInputStream): Future[String] =
    stdout.stream.startAsync.toFuture.flatMap(_.read[Any](End).toFuture).map {
      case _: Byte => "unexpected-chunk:1"
      case _       => "closed"
    }.recover {
      case error: StreamError =>
        error.value.asInstanceOf[ByteStreamFailure] match {
          case ByteStreamFailure.Cancelled         => "cancelled"
          case ByteStreamFailure.Abandoned         => "abandoned"
          case ByteStreamFailure.ResourceExhausted => "resource-exhausted"
          case ByteStreamFailure.Failed(_)         => "failed"
        }
    }
}
