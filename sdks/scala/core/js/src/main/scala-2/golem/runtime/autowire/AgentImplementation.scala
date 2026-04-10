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

import scala.language.experimental.macros

import scala.reflect.macros.blackbox

// format: off
private[golem] object AgentImplementation {

  def registerAnyCtorType[Trait](
    typeName: String,
    mode: AgentMode,
    implType: _root_.golem.runtime.AgentImplementationType[Trait, Any]
  ): AgentDefinition[Trait] =
    AgentImplementationRuntime.register(typeName, mode, implType)

  /**
   * Registers an agent implementation by class type.
   *
   * The macro inspects the Impl class constructor, separates Id params
   * from Config[T] params, and generates the registration automatically.
   * Config[T] params are excluded from agent Id and lazily loaded at runtime.
   *
   * @tparam Trait The agent trait type
   * @tparam Impl  The implementation class type
   * @return The registered agent definition
   */
  def registerClass[Trait, Impl <: Trait]: AgentDefinition[Trait] =
    macro AgentImplementationMacroFacade.registerClassImpl[Trait, Impl]
}

private[golem] object AgentImplementationMacroFacade {
  def registerClassImpl[Trait: c.WeakTypeTag, Impl: c.WeakTypeTag](c: blackbox.Context): c.Expr[AgentDefinition[Trait]] = {
    import c.universe._

    val traitType = weakTypeOf[Trait]

    val typeNameExpr = c.Expr[String](q"_root_.golem.runtime.macros.AgentNameMacro.typeName[$traitType]")

    c.Expr[AgentDefinition[Trait]](
      q"""
      {
        val implType = _root_.golem.runtime.macros.AgentImplementationMacro.implementationTypeFromClass[$traitType, ${weakTypeOf[Impl]}]
          .asInstanceOf[_root_.golem.runtime.AgentImplementationType[$traitType, Any]]
        val metadataMode = implType.metadata.mode.flatMap(_root_.golem.runtime.autowire.AgentMode.fromString)
        val effectiveMode = metadataMode.getOrElse(_root_.golem.runtime.autowire.AgentMode.Durable)
        _root_.golem.runtime.autowire.AgentImplementation.registerAnyCtorType[$traitType]($typeNameExpr, effectiveMode, implType)
      }
      """
    )
  }
}
