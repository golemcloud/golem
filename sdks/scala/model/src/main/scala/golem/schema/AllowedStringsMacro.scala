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
 * Reads the allow-list encoded by the type parameter of [[UnstructuredText]] /
 * [[UnstructuredBinary]].
 *
 * `String` means unrestricted (`None`); a string literal type or a union of
 * string literal types (`"en" | "de"`) lists the allowed values in declaration
 * order. Any other type is rejected, so the allow-list is always known to the
 * compiler and the compile-time wire metadata matches the runtime codecs.
 */
private[golem] object AllowedStringsMacro {

  def literalValues(using Quotes)(tpe: quotes.reflect.TypeRepr): Either[String, Option[List[String]]] = {
    import quotes.reflect.*

    def literals(t: TypeRepr): Option[List[String]] = t.dealias match {
      case ConstantType(StringConstant(value)) => Some(List(value))
      case OrType(left, right)                 =>
        for {
          l <- literals(left)
          r <- literals(right)
        } yield l ++ r
      case _ => None
    }

    val dealiased = tpe.dealias
    if (dealiased =:= TypeRepr.of[String]) Right(None)
    else
      literals(dealiased) match {
        case Some(values) => Right(Some(values.distinct))
        case None         =>
          Left(
            s"${tpe.show} is not a valid allow-list: use String (unrestricted), a string literal type, " +
              "or a union of string literal types such as \"image/png\" | \"image/jpeg\""
          )
      }
  }

  def languages[L: Type](using Quotes): Expr[AllowedLanguages[L]] =
    literalValues(quotes.reflect.TypeRepr.of[L]) match {
      case Right(values) => '{ new AllowedLanguages[L](${ Expr(values) }) }
      case Left(error)   => quotes.reflect.report.errorAndAbort(s"Invalid UnstructuredText language type: $error")
    }

  def mimeTypes[M: Type](using Quotes): Expr[AllowedMimeTypes[M]] =
    literalValues(quotes.reflect.TypeRepr.of[M]) match {
      case Right(values) => '{ new AllowedMimeTypes[M](${ Expr(values) }) }
      case Left(error)   => quotes.reflect.report.errorAndAbort(s"Invalid UnstructuredBinary MIME type: $error")
    }
}
