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

import java.lang.annotation.{ElementType, Retention, RetentionPolicy, Target}
import scala.annotation.StaticAnnotation

/**
 * Marks an agent trait with a Golem agent type name.
 *
 * Companion ergonomics are provided by `golem.AgentCompanion`.
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.TYPE))
final class agentDefinition(
  val typeName: String = "",
  val mode: DurabilityMode = DurabilityMode.Durable,
  val mount: String = "",
  val auth: Boolean = false,
  val cors: Array[String] = Array.empty,
  val phantomAgent: Boolean = false,
  val webhookSuffix: String = "",
  val snapshotting: String = "disabled"
) extends StaticAnnotation
