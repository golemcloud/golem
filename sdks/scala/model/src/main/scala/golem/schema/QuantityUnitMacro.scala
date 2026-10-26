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
 * Reads the [[golem.runtime.annotations.quantityUnit]] annotation of a quantity
 * marker type, so the unit spec is known to the compiler and the compile-time
 * wire metadata matches the runtime codecs.
 */
private[golem] object QuantityUnitMacro {

  def spec(using Quotes)(marker: quotes.reflect.TypeRepr): Either[String, QuantitySpec] = {
    import quotes.reflect.*

    def strings(term: Term): Option[List[String]] = term match {
      case Literal(StringConstant(value))                => Some(List(value))
      case Typed(inner, _)                               => strings(inner)
      case NamedArg(_, inner)                            => strings(inner)
      case Inlined(_, Nil, inner)                        => strings(inner)
      case Apply(TypeApply(Select(_, "apply"), _), args) => all(args)
      case Apply(Select(_, "apply"), args)               => all(args)
      case Repeated(elements, _)                         => all(elements)
      case other if other.tpe <:< TypeRepr.of[Nil.type]  => Some(Nil)
      case _                                             => None
    }
    def all(terms: List[Term]): Option[List[String]] =
      terms.foldLeft(Option(List.empty[String]))((acc, t) => acc.flatMap(a => strings(t).map(a ++ _)))

    val annotationType = TypeRepr.of[golem.runtime.annotations.quantityUnit]
    val annotation     = marker.dealias.typeSymbol.annotations.find(_.tpe <:< annotationType)
    val missing        =
      s"${marker.show} is not a quantity unit marker: annotate it with @quantityUnit(\"<base unit>\", \"<unit>\", ...)"
    val notLiterals = s"The @quantityUnit units of ${marker.show} must be string literals"
    val units       = annotation match {
      case None                                                                                 => Left(missing)
      case Some(Apply(fun, args)) if fun.symbol == annotationType.typeSymbol.primaryConstructor =>
        args match {
          case List(base, suffixes) =>
            (strings(base), strings(suffixes)) match {
              case (Some(List(b)), Some(s)) => Right(QuantitySpec(b, s, None, None))
              case _                        => Left(notLiterals)
            }
          case _ => Left(notLiterals)
        }
      case Some(Apply(_, args)) =>
        all(args) match {
          case Some(List(base))     => Right(QuantitySpec(base, Nil, None, None))
          case Some(base :: _ :: _) => Right(QuantitySpec(base, all(args).get, None, None))
          case Some(Nil)            => Left(s"The @quantityUnit annotation of ${marker.show} must list at least one unit")
          case None                 => Left(notLiterals)
        }
      case Some(_) => Left(missing)
    }
    units.flatMap { spec =>
      if (spec.baseUnit.isEmpty || spec.allowedSuffixes.contains(""))
        Left(s"The @quantityUnit units of ${marker.show} must be non-empty")
      else Right(spec)
    }
  }

  def derive[U: Type](using Quotes): Expr[QuantityUnit[U]] =
    spec(quotes.reflect.TypeRepr.of[U]) match {
      case Right(s)    => '{ new QuantityUnit[U](${ Expr(s.baseUnit) }, ${ Expr(s.allowedSuffixes) }) }
      case Left(error) => quotes.reflect.report.errorAndAbort(error)
    }
}
