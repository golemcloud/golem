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

import golem.data.GolemSchema
import golem.runtime.AgentMetadata

import scala.reflect.macros.blackbox

object AgentMacros {
  def agentMetadata[T]: AgentMetadata = macro golem.runtime.macros.AgentDefinitionMacroImpl.impl[T]
}

object AgentMacrosImpl {
  private val schemaHint: String =
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  def agentMetadataImpl[T: c.WeakTypeTag](c: blackbox.Context): c.Expr[AgentMetadata] = {
    import c.universe._

    val tpe        = weakTypeOf[T]
    val typeSymbol = tpe.typeSymbol

    if (!typeSymbol.isClass || !typeSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"@agent target must be a trait, found: ${typeSymbol.fullName}")
    }

    val descriptionType = typeOf[golem.runtime.annotations.description]
    val promptType      = typeOf[golem.runtime.annotations.prompt]

    val traitDescription = annotationString(c)(typeSymbol, descriptionType)
    val traitMode        = None

    val methods = tpe.decls.collect {
      case method: MethodSymbol if method.isAbstract && method.isMethod =>
        val methodName       = method.name.toString
        val descExpr         = optionalStringExpr(c)(annotationString(c)(method, descriptionType))
        val promptExpr       = optionalStringExpr(c)(annotationString(c)(method, promptType))
        val inputSchemaExpr  = methodInputSchema(c)(method)
        val outputSchemaExpr = methodOutputSchema(c)(method)

        q"""
          _root_.golem.runtime.MethodMetadata(
            name = $methodName,
            description = $descExpr,
            prompt = $promptExpr,
            mode = _root_.scala.None,
            input = $inputSchemaExpr,
            output = $outputSchemaExpr,
            httpEndpoints = _root_.scala.Nil
          )
        """
    }.toList

    val typeName      = typeSymbol.fullName
    val traitDescExpr = optionalStringExpr(c)(traitDescription)
    val traitModeExpr = optionalTreeExpr(c)(traitMode)

    val ctorSchema =
      idSchemaFromAgentInput(c)(tpe)

    c.Expr[AgentMetadata](q"""
      _root_.golem.runtime.AgentMetadata(
        name = $typeName,
        description = $traitDescExpr,
        mode = $traitModeExpr,
        methods = List(..$methods),
        constructor = $ctorSchema,
        httpMount = _root_.scala.None
      )
    """)
  }

  private def idSchemaFromAgentInput(c: blackbox.Context)(tpe: c.universe.Type): c.Tree = {
    import c.universe._
    val baseSymOpt = tpe.baseClasses.find(_.fullName == "golem.BaseAgent")
    val baseArgs   = baseSymOpt.toList.flatMap(sym => tpe.baseType(sym).typeArgs)
    val inputTpe   = baseArgs.headOption.getOrElse(typeOf[Unit]).dealias
    if (inputTpe =:= typeOf[Unit]) q"_root_.golem.data.StructuredSchema.Tuple(Nil)"
    else structuredSchemaExpr(c)(inputTpe)
  }

  private def methodInputSchema(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree = {
    import c.universe._

    val params = method.paramLists.flatten.collect {
      case param if param.isTerm => (param.name.toString, param.typeSignature)
    }

    if (params.isEmpty) {
      q"_root_.golem.data.StructuredSchema.Tuple(Nil)"
    } else if (params.length == 1) {
      val (_, paramType) = params.head
      structuredSchemaExpr(c)(paramType)
    } else {
      val elements = params.map { case (name, tpe) =>
        val schemaExpr = elementSchemaExpr(c)(name, tpe)
        q"_root_.golem.data.NamedElementSchema($name, $schemaExpr)"
      }
      q"_root_.golem.data.StructuredSchema.Tuple(List(..$elements))"
    }
  }

  private def structuredSchemaExpr(c: blackbox.Context)(tpe: c.universe.Type): c.Tree = {
    import c.universe._

    val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, tpe)
    val schemaInstance  = c.inferImplicitValue(golemSchemaType)

    if (schemaInstance.isEmpty) {
      c.abort(c.enclosingPosition, s"No implicit GolemSchema available for type $tpe.$schemaHint")
    }

    q"$schemaInstance.schema"
  }

  private def elementSchemaExpr(
    c: blackbox.Context
  )(@annotation.unused paramName: String, tpe: c.universe.Type): c.Tree = {
    import c.universe._

    val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, tpe)
    val schemaInstance  = c.inferImplicitValue(golemSchemaType)

    if (schemaInstance.isEmpty) {
      c.abort(c.enclosingPosition, s"No implicit GolemSchema available for type $tpe.$schemaHint")
    }

    q"$schemaInstance.elementSchema"
  }

  private def methodOutputSchema(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree =
    structuredSchemaExpr(c)(method.returnType)

  private def annotationString(
    c: blackbox.Context
  )(symbol: c.universe.Symbol, annType: c.universe.Type): Option[String] = {
    import c.universe._

    symbol.annotations.collectFirst {
      case ann if ann.tree.tpe =:= annType =>
        ann.tree.children.tail.collectFirst { case Literal(Constant(value: String)) =>
          value
        }
    }.flatten
  }

  // Note: @mode was removed; trait/method mode are sourced from @agentDefinition(mode=...) only.

  private def optionalStringExpr(c: blackbox.Context)(value: Option[String]): c.Tree = {
    import c.universe._
    value match {
      case Some(v) => q"Some($v)"
      case None    => q"None"
    }
  }

  private def optionalTreeExpr(c: blackbox.Context)(value: Option[c.Tree]): c.Tree = {
    import c.universe._
    value match {
      case Some(v) => q"Some($v)"
      case None    => q"None"
    }
  }
}
