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

package golem.runtime.annotations

/**
 * Durability mode for agents and methods.
 *
 * This is a shared Scala ADT so it works uniformly across:
 *   - Scala 2 + Scala 3
 *   - JVM + Scala.js
 *
 * The generated metadata uses the lower-case wire values ("durable",
 * "ephemeral").
 */
sealed abstract class DurabilityMode private (val wire: String) extends Product with Serializable {
  final def wireValue(): String       = wire
  override final def toString: String = wire
}

object DurabilityMode {
  case object Durable   extends DurabilityMode("durable")
  case object Ephemeral extends DurabilityMode("ephemeral")

  def fromWireValue(value: String): Option[DurabilityMode] =
    Option(value).map(_.toLowerCase) match {
      case Some("durable")   => Some(Durable)
      case Some("ephemeral") => Some(Ephemeral)
      case _                 => None
    }
}
