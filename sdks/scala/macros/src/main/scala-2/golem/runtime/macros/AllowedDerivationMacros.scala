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

import golem.data.unstructured.{AllowedLanguages, AllowedMimeTypes}

import scala.reflect.macros.blackbox

object AllowedLanguagesDerivation {
  def derived[A]: AllowedLanguages[A] = macro AllowedLanguagesDerivationMacro.derive[A]
}

object AllowedMimeTypesDerivation {
  def derived[A]: AllowedMimeTypes[A] = macro AllowedMimeTypesDerivationMacro.derive[A]
}

object AllowedLanguagesDerivationMacro {
  def derive[A: c.WeakTypeTag](c: blackbox.Context): c.Expr[AllowedLanguages[A]] = {
    import c.universe._

    val tpe    = weakTypeOf[A]
    val symbol = tpe.typeSymbol

    val languageCodeType = typeOf[golem.runtime.annotations.languageCode]

    val codes = MacroHelpers.collectEntries(c)(symbol, languageCodeType, _.toLowerCase.replace('_', '-'))

    if (codes.isEmpty) {
      c.abort(
        c.enclosingPosition,
        s"AllowedLanguages can only be derived for sealed traits with cases, found: ${symbol.fullName}"
      )
    }

    val codesExpr = codes.map(code => q"$code")

    c.Expr[AllowedLanguages[A]](q"""
      new _root_.golem.data.unstructured.AllowedLanguages[$tpe] {
        override val codes: Option[List[String]] = Some(List(..$codesExpr))
      }
    """)
  }
}

object AllowedMimeTypesDerivationMacro {
  def derive[A: c.WeakTypeTag](c: blackbox.Context): c.Expr[AllowedMimeTypes[A]] = {
    import c.universe._

    val tpe    = weakTypeOf[A]
    val symbol = tpe.typeSymbol

    val mimeTypeType = typeOf[golem.runtime.annotations.mimeType]

    val codes = MacroHelpers.collectEntries(c)(symbol, mimeTypeType, identity)

    if (codes.isEmpty) {
      c.abort(
        c.enclosingPosition,
        s"AllowedMimeTypes can only be derived for sealed traits with cases, found: ${symbol.fullName}"
      )
    }

    val codesExpr = codes.map(code => q"$code")

    c.Expr[AllowedMimeTypes[A]](q"""
      new _root_.golem.data.unstructured.AllowedMimeTypes[$tpe] {
        override val mimeTypes: Option[List[String]] = Some(List(..$codesExpr))
      }
    """)
  }
}

private object MacroHelpers {
  def collectEntries(c: blackbox.Context)(
    symbol: c.universe.Symbol,
    annotationType: c.universe.Type,
    defaultTransform: String => String
  ): List[String] = {
    val children = if (symbol.isClass && symbol.asClass.isSealed) {
      symbol.asClass.knownDirectSubclasses.toList
    } else {
      Nil
    }

    if (children.isEmpty) Nil
    else {
      children.map { child =>
        annotationValue(c)(child, annotationType).getOrElse(defaultTransform(child.name.toString))
      }
    }
  }

  def annotationValue(c: blackbox.Context)(
    symbol: c.universe.Symbol,
    annotationType: c.universe.Type
  ): Option[String] = {
    import c.universe._

    symbol.annotations.collectFirst {
      case ann if ann.tree.tpe =:= annotationType =>
        ann.tree.children.tail.collectFirst { case Literal(Constant(value: String)) =>
          value
        }
    }.flatten
  }
}
