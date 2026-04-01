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

// Macro annotations live in a separate module; do not depend on them here.
import golem.runtime.AgentType
import golem.AgentApi

import scala.quoted.*

object AgentSdkMacro {
  transparent inline def derived[Trait]: AgentApi[Trait] =
    ${ derivedImpl[Trait] }

  private def derivedImpl[Trait: Type](using Quotes): Expr[AgentApi[Trait]] = {
    import quotes.reflect.*

    val traitSym = TypeRepr.of[Trait].typeSymbol

    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name

    def extractTypeNameFromAgentDefinition(sym: Symbol): Option[String] =
      sym.annotations.collectFirst {
        case Apply(Select(New(tpt), _), args)
            if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
          args.collectFirst {
            case Literal(StringConstant(value))                       => value
            case NamedArg("typeName", Literal(StringConstant(value))) => value
          }.map(_.trim).filter(_.nonEmpty)
      }.flatten

    val hasAnn =
      traitSym.annotations.exists {
        case Apply(Select(New(tpt), _), _)
            if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
          true
        case _ => false
      }

    val typeName =
      extractTypeNameFromAgentDefinition(traitSym).map(validateTypeName).getOrElse {
        if !hasAnn then report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${traitSym.fullName}")
        defaultTypeNameFromTrait(traitSym)
      }
    val typeNameExpr = Expr(typeName)

    val ctorTypeRepr = {
      val traitRepr = TypeRepr.of[Trait]
      val baseSym   = traitRepr.baseClasses.find(_.fullName == "golem.BaseAgent").getOrElse(Symbol.noSymbol)
      if (baseSym == Symbol.noSymbol) TypeRepr.of[Unit]
      else
        traitRepr.baseType(baseSym) match {
          case AppliedType(_, List(arg)) => arg
          case _                         => TypeRepr.of[Unit]
        }
    }

    ctorTypeRepr.asType match {
      case '[ctor] =>
        '{
          new AgentApi[Trait] {
            override type Id = ctor
            override val typeName: String                  = $typeNameExpr
            override val agentType: AgentType[Trait, ctor] =
              golem.runtime.macros.AgentClientMacro
                .agentType[Trait]
                .asInstanceOf[AgentType[Trait, ctor]]
          }
        }
    }
  }

  private def validateTypeName(value: String): String =
    value
}
