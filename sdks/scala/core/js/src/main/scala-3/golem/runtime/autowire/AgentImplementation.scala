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

import golem.runtime.macros.AgentImplementationMacro
import golem.runtime.macros.AgentNameMacro

private[golem] object AgentImplementation {

  def registerAnyCtorType[Trait](
    typeName: String,
    mode: AgentMode,
    implType: golem.runtime.AgentImplementationType[Trait, _]
  ): AgentDefinition[Trait] =
    AgentImplementationRuntime.register(
      typeName,
      mode,
      implType.asInstanceOf[golem.runtime.AgentImplementationType[Trait, Any]]
    )

  /**
   * Registers an agent implementation by class type.
   *
   * The macro inspects the Impl class Id, separates identity params from
   * Config[T] params, and generates the registration automatically. Config[T]
   * params are excluded from agent identity and lazily loaded at runtime.
   *
   * @tparam Trait
   *   The agent trait type
   * @tparam Impl
   *   The implementation class type
   * @return
   *   The registered agent definition
   */
  inline def registerClass[Trait, Impl <: Trait]: AgentDefinition[Trait] = {
    val implType      = AgentImplementationMacro.implementationTypeFromClass[Trait, Impl]
    val metadataMode  = implType.metadata.mode.flatMap(AgentMode.fromString)
    val effectiveMode = metadataMode.getOrElse(AgentMode.Durable)
    val typeName      = AgentNameMacro.typeName[Trait]
    registerAnyCtorType(typeName, effectiveMode, implType)
  }
}
