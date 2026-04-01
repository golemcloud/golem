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

import scala.quoted.*

object AgentNameMacro {
  inline def typeName[T]: String =
    ${ typeNameImpl[T] }

  private def typeNameImpl[T: Type](using Quotes): Expr[String] = {
    import quotes.reflect.*
    val sym = TypeRepr.of[T].typeSymbol

    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name

    def extractAgentDefinitionTypeName(args: List[Term]): Option[String] =
      args.collectFirst {
        case Literal(StringConstant(value))                       => value
        case NamedArg("typeName", Literal(StringConstant(value))) => value
      }

    val maybe = sym.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        extractAgentDefinitionTypeName(args)
    }.flatten

    maybe match {
      case Some(value) if value.trim.nonEmpty => Expr(value)
      case _                                  =>
        // If @agentDefinition is present but typeName was omitted/empty, derive a stable default.
        // This keeps user code minimal while still requiring an explicit marker annotation.
        val hasAnn =
          sym.annotations.exists {
            case Apply(Select(New(tpt), _), _)
                if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
              true
            case _ => false
          }
        if !hasAnn then report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${sym.fullName}")
        Expr(defaultTypeNameFromTrait(sym))
    }
  }

}
