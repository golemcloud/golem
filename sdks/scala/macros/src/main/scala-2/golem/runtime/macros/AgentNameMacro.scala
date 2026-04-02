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

object AgentNameMacro {
  def typeName[T]: String = macro AgentNameMacroImpl.typeNameImpl[T]
}

object AgentNameMacroImpl {
  def typeNameImpl[T: c.WeakTypeTag](c: blackbox.Context): c.Expr[String] = {
    import c.universe._

    val tpe = weakTypeOf[T]
    val sym = tpe.typeSymbol

    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name.decodedName.toString

    val maybe = sym.annotations.collectFirst {
      case ann
          if ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        ann.tree.children.tail.collectFirst { case Literal(Constant(s: String)) => s }
    }.flatten

    maybe match {
      case Some(value) if value.trim.nonEmpty => c.Expr[String](Literal(Constant(value)))
      case _                                  =>
        // In Scala 2, macro annotations are stripped after expansion, so the trait may not
        // retain @agentDefinition. The macro annotation injects a `def typeName: String`
        // into the companion; use that as a fallback.
        val companion0 = sym.companion
        val companion1 =
          if (companion0 != NoSymbol) companion0
          else {
            // Nested traits (e.g. inside an object) sometimes don't have a companion set at macro time.
            val owner = sym.owner
            if (owner != NoSymbol) owner.typeSignature.member(sym.name.toTermName)
            else NoSymbol
          }

        val companion2 =
          if (companion1 != NoSymbol && companion1.isModule) companion1
          else {
            // Last-resort lookup by fully-qualified name (works for nested objects too).
            try c.mirror.staticModule(sym.fullName)
            catch { case _: Throwable => NoSymbol }
          }

        if (companion2 != NoSymbol && companion2.isModule) {
          // Prefer checking the module class (more reliable than the singleton type's decls),
          // but ultimately just emit `<companion>.typeName` and let the typer decide.
          val _   = companion2.asModule.moduleClass.typeSignature.decls // force completion
          val ref = Ident(companion2.asModule)
          c.Expr[String](q"$ref.typeName")
        } else {
          // If the trait had @agentDefinition but the typeName was omitted/empty, derive a default.
          val hasAnn =
            sym.annotations.exists(ann =>
              ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition"
            )
          if (!hasAnn) c.abort(c.enclosingPosition, s"Missing @agentDefinition(...) on agent trait: ${sym.fullName}")
          c.Expr[String](Literal(Constant(defaultTypeNameFromTrait(sym))))
        }
    }
  }

}
