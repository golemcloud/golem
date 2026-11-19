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

package golem.runtime.macros

import golem.LiveValues

import scala.quoted.*

/**
 * Rule 8: a `Principal`, a `Caller` and a `TokenProvenance` are live values,
 * never data. A method or constructor parameter, a result, an agent id, a tool
 * signature or a value type that can reach one fails the build.
 */
private[macros] object LiveValueCheck {

  /** Whether a live value can be reached from `tpe`. */
  def contains(using Quotes)(tpe: quotes.reflect.TypeRepr): Boolean = {
    import quotes.reflect.*

    val visited = scala.collection.mutable.Set.empty[Symbol]

    def isLibrary(symbol: Symbol): Boolean = {
      val name = symbol.fullName
      name.startsWith("scala.") || name.startsWith("java.") || name.startsWith("zio.")
    }

    def walk(t: TypeRepr, depth: Int): Boolean =
      if (depth > 8) false
      else
        t.dealias match {
          case OrType(left, right)  => walk(left, depth + 1) || walk(right, depth + 1)
          case AndType(left, right) => walk(left, depth + 1) || walk(right, depth + 1)
          case other                =>
            val symbol = other.typeSymbol
            if (LiveValues.TypeNames.contains(symbol.fullName)) true
            else {
              val args = other match {
                case AppliedType(_, arguments) => arguments.exists(argument => walk(argument, depth + 1))
                case _                         => false
              }
              args || (!isLibrary(symbol) && symbol.isClassDef && visited.add(symbol) && {
                // Fields of a case class are part of the value it carries.
                symbol.caseFields.exists { field =>
                  field.tree match {
                    case v: ValDef => walk(other.memberType(field), depth + 1) || walk(v.tpt.tpe, depth + 1)
                    case _         => false
                  }
                }
              })
            }
        }

    walk(tpe, 0)
  }

  /** Fails the build when a live value can be reached from `tpe`. */
  def reject(using Quotes)(position: String, tpe: quotes.reflect.TypeRepr): Unit =
    if (contains(tpe)) quotes.reflect.report.errorAndAbort(s"$position: ${LiveValues.Message}")
}
