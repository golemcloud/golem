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

import java.util.concurrent.atomic.AtomicReference
import scala.concurrent.ExecutionContext
import scala.concurrent.duration._

/**
 * Connection configuration shared by all generated bridge clients in this
 * project. Holds the target [[GolemServer]], the application/environment names
 * the agents live in, and the [[ExecutionContext]] used to complete the
 * Future-based client calls.
 *
 * `reconnectWindow` is how long a streaming call keeps reconnecting while the
 * server accepts neither its start nor a resume; after it the call fails with
 * a [[BridgeUnreachableException]]. `Duration.Inf` removes the limit.
 */
final case class Configuration(
  server: GolemServer,
  appName: String,
  envName: String,
  executionContext: ExecutionContext,
  reconnectWindow: Duration = Configuration.DefaultReconnectWindow
) {
  require(
    reconnectWindow == Duration.Inf || (reconnectWindow.isFinite && reconnectWindow > Duration.Zero),
    "reconnectWindow must be positive or Duration.Inf"
  )
}

object Configuration {
  /** The reconnect window of a configuration that does not set one. */
  val DefaultReconnectWindow: FiniteDuration = 5.minutes

  // Replaceable so REPL sessions and tests can reconfigure the target server
  // without restarting the process. The Rust/TS bridges use a write-once cell;
  // here we deliberately allow replacement.
  private val current = new AtomicReference[Option[Configuration]](None)

  /**
   * Configure all bridge clients in this project. Can be called repeatedly to
   * point the clients at a different server.
   */
  def configure(
    server: GolemServer,
    appName: String,
    envName: String,
    executionContext: ExecutionContext = ExecutionContext.global,
    reconnectWindow: Duration = DefaultReconnectWindow
  ): Unit =
    current.set(Some(Configuration(server, appName, envName, executionContext, reconnectWindow)))

  /** Replace the active configuration wholesale. */
  def set(configuration: Configuration): Unit =
    current.set(Some(configuration))

  /** The active configuration, or a failure if [[configure]] was never called. */
  def get: Configuration =
    current.get() match {
      case Some(configuration) => configuration
      case None                =>
        throw BridgeException(
          "Golem bridge configuration has not been set. Call Configuration.configure(...) first."
        )
    }

  /** The active configuration, if any. */
  def getOption: Option[Configuration] = current.get()

  def isConfigured: Boolean = current.get().isDefined
}
