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

package golem

import golem.runtime.AgentType

/**
 * Pure metadata + reflected type for an agent trait.
 *
 * This lives in `model` so macros can derive it without depending on the
 * runtime (`core`).
 */
trait AgentApi[Trait] {
  type Id

  /** Golem agent type name, from `@agentDefinition("...")`. */
  def typeName: String

  /** Reflected agent type (schemas + function names). */
  def agentType: AgentType[Trait, Id]
}
