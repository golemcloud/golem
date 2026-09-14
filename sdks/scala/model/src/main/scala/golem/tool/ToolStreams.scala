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

import scala.concurrent.{ExecutionContext, Future}

/**
 * Opaque handle to the byte stream supplied as a tool invocation's stdin. A
 * tool method parameter of this type is auto-injected from the invocation and
 * excluded from the tool's input schema. The platform layer (Scala.js guest)
 * provides the concrete implementation carrying the underlying WASI stream.
 */
trait ToolInputStream {

  /** Reads the next chunk. `Right(None)` is clean EOF. */
  def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
    Future.failed(new UnsupportedOperationException("tool input stream is not readable"))

  /** Stops further consumption and releases any blocked read. */
  def cancel(): Future[Unit]               = close()
  private[golem] def close(): Future[Unit] = Future.successful(())
}

/**
 * Opaque handle to the process stdout stream a tool invocation may write to. A
 * tool method parameter of this type is auto-injected and excluded from the
 * tool's input schema. The caller receives the paired stream independently from
 * the structured result.
 */
trait ToolOutputStream {
  def write(bytes: Array[Byte]): Future[Either[StreamWriteError, Unit]] =
    Future.failed(new UnsupportedOperationException("tool output stream is not writable"))
  def finish(): Future[Either[StreamWriteError, Unit]] =
    Future.failed(new UnsupportedOperationException("tool output stream is not finishable"))
  def fail(reason: ByteStreamFailure): Future[Either[StreamWriteError, Unit]] =
    Future.failed(new UnsupportedOperationException("tool output stream is not failable"))
  private[golem] def close(): Future[Unit] = Future.successful(())
}

/**
 * Transfer-only handle to the stdin of a tool invocation passing through
 * middleware. Middleware may forward this handle to an underlying tool but
 * cannot read from it.
 */
trait ToolMiddlewareInputHandle {
  private[golem] def close(): Future[Unit] = Future.successful(())
}

/**
 * Transfer-only handle to the stdout returned by an underlying tool. Middleware
 * may return this handle from its own invocation but cannot write to it.
 */
trait ToolMiddlewareOutputHandle {
  private[golem] def close(): Future[Unit] = Future.successful(())
}

sealed trait ByteStreamFailure extends Product with Serializable
object ByteStreamFailure {
  case object Cancelled                    extends ByteStreamFailure
  case object Abandoned                    extends ByteStreamFailure
  case object ResourceExhausted            extends ByteStreamFailure
  final case class Failed(message: String) extends ByteStreamFailure
}

sealed trait ByteStreamCloseCause extends Product with Serializable
object ByteStreamCloseCause {
  case object Finished                               extends ByteStreamCloseCause
  final case class Failed(reason: ByteStreamFailure) extends ByteStreamCloseCause
  case object ConsumerCancelled                      extends ByteStreamCloseCause
}

sealed trait StreamWriteError extends Product with Serializable
object StreamWriteError {
  final case class Closed(cause: ByteStreamCloseCause) extends StreamWriteError
  case object ConcurrentOperation                      extends StreamWriteError
}

/**
 * A started stdout-bearing invocation. Stream and result have independent
 * lifetimes.
 */
final case class ToolInvocation[+E, +A](
  stdout: ToolInputStream,
  result: Future[Either[ToolError[E], A]],
  cancel: () => Unit
) {

  /** Drains stdout concurrently with the structured result. */
  def collect()(implicit ec: ExecutionContext): Future[Either[ToolError[E], (A, Array[Byte])]] = {
    def drain(chunks: Vector[Array[Byte]]): Future[Array[Byte]] =
      stdout.read().flatMap {
        case Right(Some(chunk)) => drain(chunks :+ chunk)
        case Right(None)        => Future.successful(chunks.flatten.toArray)
        case Left(failure)      => Future.failed(new ToolStreamException(failure))
      }
    val terminal = result.map(Right(_): Either[Throwable, Either[ToolError[E], A]]).recover { case t => Left(t) }
    val output   = drain(Vector.empty).map(Right(_): Either[Throwable, Array[Byte]]).recover { case t => Left(t) }
    terminal.zip(output).flatMap {
      case (Right(Left(error @ ToolError.Tool(_))), _) => Future.successful(Left(error))
      case (Left(error), _)                            => Future.failed(error)
      case (_, Left(error))                            => Future.failed(error)
      case (Right(result), Right(bytes))               => Future.successful(result.map(_ -> bytes))
    }
  }
}

final class ToolStreamException(val failure: ByteStreamFailure)
    extends RuntimeException(s"tool byte stream failed: $failure")
