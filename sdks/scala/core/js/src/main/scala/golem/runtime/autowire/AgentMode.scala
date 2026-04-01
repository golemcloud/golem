/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.autowire

/**
 * Defines the persistence mode for an agent.
 *
 * Agent modes affect how state is managed across invocations:
 *   - [[AgentMode.Durable]] - State persists across invocations (default)
 *   - [[AgentMode.Ephemeral]] - Fresh instance per invocation
 */
sealed trait AgentMode {

  /** The string value used in annotations and serialization. */
  def value: String
}

object AgentMode {

  /**
   * Parses an agent mode from its string representation.
   *
   * @param value
   *   The mode string (case-insensitive)
   * @return
   *   The parsed mode, or None if invalid
   */
  def fromString(value: String): Option[AgentMode] =
    Option(value).map(_.toLowerCase) match {
      case Some("durable")   => Some(Durable)
      case Some("ephemeral") => Some(Ephemeral)
      case _                 => None
    }

  /**
   * Durable mode - agent state persists across invocations.
   *
   * This is the default mode. Use when agents need to maintain state between
   * method calls.
   */
  case object Durable extends AgentMode {
    override val value: String = "durable"
  }

  /**
   * Ephemeral mode - fresh agent instance per invocation.
   *
   * Use for stateless agents or when each invocation should start with a clean
   * slate.
   */
  case object Ephemeral extends AgentMode {
    override val value: String = "ephemeral"
  }
}
