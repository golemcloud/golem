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

package golem.bridge.runtime

import scala.concurrent.duration.FiniteDuration

/**
 * Error raised by the generated Golem bridge client. Future-based client
 * methods surface failures as a failed `Future` carrying this exception.
 */
class BridgeException(message: String, cause: Throwable)
    extends RuntimeException(message, cause)

/**
 * A streaming call that could not reach the server within its reconnect
 * window. `lastCause` is the last failure: a `WebSocketHandshakeException` for
 * a refused upgrade, a [[BridgeException]] for a retryable rejection, the
 * connection error, or a `TimeoutException` for a start or resume the server
 * never answered.
 */
final class BridgeUnreachableException(
  val endpoint: String,
  val reconnectWindow: FiniteDuration,
  val lastCause: Throwable
) extends BridgeException(
      s"$endpoint was unreachable for $reconnectWindow: ${lastCause.getMessage}",
      lastCause
    )

/** A streaming call its [[CancellationToken]] cancelled. */
final class BridgeCancelledException extends BridgeException("the invocation was cancelled", null)

object BridgeException {
  def apply(message: String): BridgeException             = new BridgeException(message, null)
  def apply(message: String, cause: Throwable): BridgeException = new BridgeException(message, cause)
}
