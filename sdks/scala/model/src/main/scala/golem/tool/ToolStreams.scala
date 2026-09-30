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

import zio.blocks.async.*
import zio.blocks.streams.Stream

import scala.concurrent.{ExecutionContext, Future}
import scala.util.Success

/**
 * Opaque handle to the byte stream supplied as a tool invocation's stdin. A
 * tool method parameter of this type is auto-injected from the invocation and
 * excluded from the tool's input schema. The platform layer (Scala.js guest)
 * provides the concrete implementation carrying the underlying WASI stream.
 */
trait ToolInputStream {

  /**
   * The invocation-scoped byte stream. It must be materialized at most once.
   */
  def stream: Stream[ByteStreamFailure, Byte]

  /** Stops further consumption and releases any blocked read. */
  def cancel(): Future[Unit]
  private[golem] def close(): Future[Unit] = Future.successful(())
}

/**
 * Writable handle to the host-supplied, invocation-scoped stdout stream. A tool
 * method parameter of this type is auto-injected and excluded from the tool's
 * input schema. The caller receives the paired stream independently from the
 * structured result. Await writes before returning; overlapping operations fail
 * with `ConcurrentOperation`. The guest finishes an open writer on return and
 * releases it, preserving an explicitly selected finish or failure.
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
  private[golem] def drained: Future[Unit] = Future.successful(())
}

final case class ToolMiddlewareOutputs[+A](
  result: A,
  stdout: Option[ToolMiddlewareOutputHandle],
  stderr: Option[ToolMiddlewareOutputHandle]
)

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
 * A started output-bearing invocation. Streams and result have independent
 * lifetimes.
 */
final case class ToolInvocation[+E, +A](
  stdout: Option[ToolInputStream],
  stderr: Option[ToolInputStream],
  result: Future[Either[ToolError[E], A]],
  cancel: () => Unit
) {

  /** Drains both outputs concurrently with the structured result. */
  def collect()(implicit ec: ExecutionContext): Future[CollectedToolInvocation[E, A]] = {
    def collectOutput(stream: Option[ToolInputStream]): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
      stream
        .fold(Future.successful(Right(Option.empty[Array[Byte]]): Either[ByteStreamFailure, Option[Array[Byte]]]))(
          value => value.stream.runCollectAsync.toFuture.map(_.map(bytes => Some(bytes.toArray)))
        )

    result.transform(Success(_)).zip(collectOutput(stdout)).zip(collectOutput(stderr)).map {
      case ((result, stdout), stderr) => CollectedToolInvocation(result.get, stdout, stderr)
    }
  }
}

final case class CollectedToolInvocation[+E, +A](
  result: Either[ToolError[E], A],
  stdout: Either[ByteStreamFailure, Option[Array[Byte]]],
  stderr: Either[ByteStreamFailure, Option[Array[Byte]]]
)
