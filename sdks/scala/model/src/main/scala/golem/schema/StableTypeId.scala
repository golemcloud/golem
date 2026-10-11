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

package golem.schema

import scala.quoted.*

/**
 * Renders the stable type id under which compile-time derivations cache and
 * name a type.
 *
 * Literal and union type arguments are part of the id, so
 * `Attachment["image/png"]` and `Attachment["image/jpeg"]` (or
 * `UnstructuredBinary["image/png"]` and `UnstructuredBinary["image/jpeg"]`)
 * never share a codec or a schema definition. The rendering is the one the
 * runtime derivation produces from zio-blocks type ids: class aliases are
 * expanded, aliases of literal or union types keep their alias path, string
 * literals are quoted and escaped, and union members are flattened and joined
 * with `" | "` in declaration order.
 */
private[golem] object StableTypeId {

  def of(using Quotes)(tpe: quotes.reflect.TypeRepr): String = {
    import quotes.reflect.*

    def path(symbol: Symbol): String = symbol.fullName.replace("$.", ".").stripSuffix("$")

    def escape(value: String, quote: String): String =
      value
        .replace("\\", "\\\\")
        .replace(quote, "\\" + quote)
        .replace("\n", "\\n")
        .replace("\t", "\\t")
        .replace("\r", "\\r")

    def literal(constant: Constant): String = constant match {
      case StringConstant(value) => "\"" + escape(value, "\"") + "\""
      case CharConstant(value)   => "'" + escape(value.toString, "'") + "'"
      case LongConstant(value)   => s"${value}L"
      case FloatConstant(value)  => s"${value}f"
      case other                 => String.valueOf(other.value)
    }

    def isLiteralLike(t: TypeRepr): Boolean = t match {
      case ConstantType(_) | OrType(_, _) => true
      case _                              => false
    }

    def unionMembers(t: TypeRepr): List[TypeRepr] = t match {
      case OrType(left, right) => unionMembers(left) ++ unionMembers(right)
      case other               => List(other)
    }

    def unionMember(t: TypeRepr): String = t match {
      case ConstantType(constant) => literal(constant)
      case other                  => other.typeSymbol.name
    }

    def render(t: TypeRepr): String = t match {
      case ref: TypeRef if ref.typeSymbol.isAliasType && isLiteralLike(ref.dealias) => path(ref.typeSymbol)
      case _                                                                        =>
        t.dealias match {
          case ConstantType(constant) => literal(constant)
          case union: OrType          => unionMembers(union).map(unionMember).mkString(" | ")
          case other                  =>
            val args = other.typeArgs
            path(other.typeSymbol) + (if (args.isEmpty) "" else args.map(render).mkString("<", ",", ">"))
        }
    }

    render(tpe)
  }
}
