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

package golem.runtime.macros

import scala.reflect.macros.blackbox

object AgentSdkMacro {
  def derived[Trait]: _root_.golem.AgentApi[Trait] = macro AgentSdkMacroImpl.derivedImpl[Trait]
}

object AgentSdkMacroImpl {
  def derivedImpl[Trait: c.WeakTypeTag](c: blackbox.Context): c.Expr[_root_.golem.AgentApi[Trait]] = {
    import c.universe._

    val traitTpe = weakTypeOf[Trait]
    val traitSym = traitTpe.typeSymbol

    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name.decodedName.toString

    val agentDefinitionFQN                             = "golem.runtime.annotations.agentDefinition"
    def isAgentDefinitionAnn(ann: Annotation): Boolean =
      ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN
    val rawTypeName: String =
      traitSym.annotations.collectFirst {
        case ann if isAgentDefinitionAnn(ann) =>
          ann.tree.children.tail.collectFirst { case Literal(Constant(s: String)) => s }.getOrElse("")
      }
        .map(_.trim)
        .filter(_.nonEmpty)
        .getOrElse {
          val hasAnn = traitSym.annotations.exists(a => isAgentDefinitionAnn(a))
          if (!hasAnn)
            c.abort(c.enclosingPosition, s"Missing @agentDefinition(...) on agent trait: ${traitSym.fullName}")
          defaultTypeNameFromTrait(traitSym)
        }
    val typeName: String = validateTypeName(rawTypeName)

    val ctorTpe: Type = {
      val baseSymOpt = traitTpe.baseClasses.find(_.fullName == "golem.BaseAgent")
      val baseArgs   = baseSymOpt.toList.flatMap(sym => traitTpe.baseType(sym).typeArgs)
      baseArgs.headOption.getOrElse(typeOf[Unit]).dealias
    }

    c.Expr[_root_.golem.AgentApi[Trait]](
      q"""
      new _root_.golem.AgentApi[$traitTpe] {
        override type Id = $ctorTpe
        override val typeName: String = $typeName
        override val agentType: _root_.golem.runtime.AgentType[$traitTpe, $ctorTpe] =
          _root_.golem.runtime.macros.AgentClientMacro
            .agentType[$traitTpe]
            .asInstanceOf[_root_.golem.runtime.AgentType[$traitTpe, $ctorTpe]]
      }
      """
    )
  }

  private def validateTypeName(value: String): String =
    value
}
