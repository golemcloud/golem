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
import golem.runtime.AgentType
import scala.reflect.macros.blackbox

object AgentClientMacro {
  def agentType[Trait]: AgentType[Trait, _] = macro AgentClientMacroImpl.agentTypeImpl[Trait]
}

object AgentClientMacroImpl {
  private val schemaHint: String =
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  def agentTypeImpl[Trait: c.WeakTypeTag](c: blackbox.Context): c.Expr[AgentType[Trait, _]] = {
    import c.universe._

    val traitType   = weakTypeOf[Trait]
    val traitSymbol = traitType.typeSymbol

    if (!traitSymbol.isClass || !traitSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"Agent client target must be a trait, found: ${traitSymbol.fullName}")
    }

    val (constructorType, constructorTypeExpr) = buildConstructorType(c)(traitType)
    val methods                                = buildMethods(c)(traitType)
    val traitName                              = agentTypeNameOrDefault(c)(traitSymbol)

    c.Expr[AgentType[Trait, _]](q"""
      _root_.golem.runtime.AgentType[$traitType, $constructorType](
        traitClassName = ${Literal(Constant(traitSymbol.fullName))},
        typeName = $traitName,
        constructor = $constructorTypeExpr.asInstanceOf[_root_.golem.runtime.ConstructorType[$constructorType]],
        methods = List(..$methods)
      )
    """)
  }

  private def buildConstructorType(c: blackbox.Context)(traitType: c.universe.Type): (c.universe.Type, c.Tree) = {
    import c.universe._

    val inputType  = agentInputType(c)(traitType)
    val accessMode = if (inputType =:= typeOf[Unit]) ParamAccessMode.NoArgs else ParamAccessMode.SingleArg
    val params     = if (accessMode == ParamAccessMode.SingleArg) List((TermName("args"), inputType)) else Nil

    val schemaExpr = accessMode match {
      case ParamAccessMode.MultiArgs =>
        multiParamSchemaExpr(c)("new", params)
      case _ =>
        val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, inputType)
        val schemaInstance  = c.inferImplicitValue(golemSchemaType)
        if (schemaInstance.isEmpty) {
          c.abort(
            c.enclosingPosition,
            s"Unable to summon GolemSchema for constructor input with type $inputType.$schemaHint"
          )
        }
        schemaInstance
    }

    val typeExpr = q"_root_.golem.runtime.ConstructorType[$inputType]($schemaExpr)"
    (inputType, typeExpr)
  }

  private def agentTypeNameOrDefault(c: blackbox.Context)(symbol: c.universe.Symbol): String = {
    import c.universe._
    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name.decodedName.toString

    def extractTypeName(args: List[Tree]): Option[String] =
      // Keep this simple and resilient across Scala 2 minor versions:
      // `@agentDefinition(typeName = "...")` always contains exactly one String literal (the type name).
      args.collectFirst { case Literal(Constant(s: String)) => s }

    val annOpt =
      symbol.annotations.collectFirst {
        case ann
            if ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
          ann
      }

    annOpt match {
      case None =>
        c.abort(c.enclosingPosition, s"Missing @agentDefinition(...) on agent trait: ${symbol.fullName}")
      case Some(ann) =>
        extractTypeName(ann.tree.children.tail) match {
          case Some(s) if s.trim.nonEmpty => validateTypeName(s)
          case _                          => defaultTypeNameFromTrait(symbol)
        }
    }
  }

  private def validateTypeName(value: String): String =
    value

  private def agentInputType(c: blackbox.Context)(traitType: c.universe.Type): c.universe.Type = {
    import c.universe._
    val idAnnotationType = typeOf[golem.runtime.annotations.id]

    val annotatedClass = traitType.members.collectFirst {
      case sym
          if sym.isClass && !sym.isMethod &&
            sym.annotations.exists(ann => ann.tree.tpe != null && ann.tree.tpe =:= idAnnotationType) =>
        sym
    }

    val constructorClass = annotatedClass.orElse {
      val byName = traitType.member(TypeName("Id"))
      if (byName == NoSymbol) None else Some(byName)
    }.getOrElse {
      c.abort(
        c.enclosingPosition,
        s"Agent trait ${traitType.typeSymbol.fullName} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
      )
    }
    val primaryCtor = constructorClass.asClass.primaryConstructor.asMethod
    val params      = primaryCtor.paramLists.flatten.filter(_.isTerm).map(_.typeSignature)
    params match {
      case Nil      => typeOf[Unit]
      case p :: Nil => p
      case ps       =>
        val tupleClass = rootMirror.staticClass(s"scala.Tuple${ps.length}")
        appliedType(tupleClass.toType, ps)
    }
  }

  private def buildMethods(c: blackbox.Context)(
    traitType: c.universe.Type
  ): List[c.Tree] = {
    import c.universe._

    traitType.decls.collect {
      case method: MethodSymbol if method.isAbstract && method.isMethod && method.name.toString != "new" =>
        buildMethod(c)(traitType, method)
    }.toList
  }

  private def buildMethod(
    c: blackbox.Context
  )(traitType: c.universe.Type, method: c.universe.MethodSymbol): c.Tree = {
    import c.universe._

    val methodName   = method.name.toString
    val functionName = methodName
    val metadataExpr = methodMetadata(c)(method)

    val params                       = extractParameters(c)(method)
    val accessMode                   = paramAccessMode(params)
    val inputType                    = inputTypeFor(c)(accessMode, params)
    val (invocationKind, outputType) = methodInvocationInfo(c)(method)

    val invocationExpr = invocationKind match {
      case InvocationKind.Awaitable     => q"_root_.golem.runtime.MethodInvocation.Awaitable"
      case InvocationKind.FireAndForget => q"_root_.golem.runtime.MethodInvocation.FireAndForget"
    }

    val inputSchemaExpr = accessMode match {
      case ParamAccessMode.MultiArgs =>
        multiParamSchemaExpr(c)(methodName, params)
      case _ =>
        val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, inputType)
        val schemaInstance  = c.inferImplicitValue(golemSchemaType)
        if (schemaInstance.isEmpty) {
          c.abort(
            c.enclosingPosition,
            s"Unable to summon GolemSchema for input of method $methodName with type $inputType.$schemaHint"
          )
        }
        schemaInstance
    }

    val golemOutputSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, outputType)
    val outputSchemaInstance  = c.inferImplicitValue(golemOutputSchemaType)
    if (outputSchemaInstance.isEmpty) {
      c.abort(
        c.enclosingPosition,
        s"Unable to summon GolemSchema for output of method $methodName with type $outputType.$schemaHint"
      )
    }

    q"""
      _root_.golem.runtime.AgentMethod[$traitType, $inputType, $outputType](
        metadata = $metadataExpr,
        functionName = $functionName,
        inputSchema = $inputSchemaExpr,
        outputSchema = $outputSchemaInstance,
        invocation = $invocationExpr
      )
    """
  }

  private def methodMetadata(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree = {
    import c.universe._

    val methodName   = method.name.toString
    val inputSchema  = methodInputSchema(c)(method)
    val outputSchema = methodOutputSchema(c)(method)

    q"""
      _root_.golem.runtime.MethodMetadata(
        name = $methodName,
        description = None,
        prompt = None,
        mode = None,
        input = $inputSchema,
        output = $outputSchema
      )
    """
  }

  private def methodInputSchema(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree = {
    import c.universe._

    val params = extractParameters(c)(method)

    if (params.isEmpty) {
      q"_root_.golem.data.StructuredSchema.Tuple(Nil)"
    } else if (params.length == 1) {
      val (_, paramType) = params.head
      structuredSchemaExpr(c)(paramType)
    } else {
      val elements = params.map { case (name, tpe) =>
        val schemaExpr = elementSchemaExpr(c)(name.toString, tpe)
        q"_root_.golem.data.NamedElementSchema(${name.toString}, $schemaExpr)"
      }
      q"_root_.golem.data.StructuredSchema.Tuple(List(..$elements))"
    }
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

  private def methodOutputSchema(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree = {
    val outputType = unwrapAsyncType(c)(method.returnType)
    structuredSchemaExpr(c)(outputType)
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

  private def unwrapAsyncType(c: blackbox.Context)(tpe: c.universe.Type): c.universe.Type = {
    import c.universe._

    val futureSymbol = typeOf[scala.concurrent.Future[_]].typeSymbol

    tpe match {
      case TypeRef(_, sym, args) if sym == futureSymbol && args.nonEmpty =>
        args.head
      case TypeRef(_, sym, args) if sym.fullName == "scala.scalajs.js.Promise" && args.nonEmpty =>
        args.head
      case _ =>
        tpe
    }
  }

  private def extractParameters(c: blackbox.Context)(
    method: c.universe.MethodSymbol
  ): List[(c.universe.TermName, c.universe.Type)] =
    method.paramLists.flatten.collect {
      case param if param.isTerm => (param.name.toTermName, param.typeSignature)
    }

  private def paramAccessMode(params: List[(_, _)]): ParamAccessMode = params match {
    case Nil      => ParamAccessMode.NoArgs
    case _ :: Nil => ParamAccessMode.SingleArg
    case _        => ParamAccessMode.MultiArgs
  }

  private def inputTypeFor(
    c: blackbox.Context
  )(accessMode: ParamAccessMode, params: List[(c.universe.TermName, c.universe.Type)]): c.universe.Type = {
    import c.universe._
    accessMode match {
      case ParamAccessMode.NoArgs    => typeOf[Unit]
      case ParamAccessMode.SingleArg => params.head._2
      case ParamAccessMode.MultiArgs => typeOf[Vector[Any]]
    }
  }

  private def methodInvocationInfo(
    c: blackbox.Context
  )(method: c.universe.MethodSymbol): (InvocationKind, c.universe.Type) = {
    import c.universe._

    val returnType   = method.returnType
    val futureSymbol = typeOf[scala.concurrent.Future[_]].typeSymbol

    returnType match {
      case TypeRef(_, sym, args) if sym == futureSymbol && args.nonEmpty =>
        (InvocationKind.Awaitable, args.head)
      case TypeRef(_, sym, args) if sym.fullName == "scala.scalajs.js.Promise" && args.nonEmpty =>
        (InvocationKind.Awaitable, args.head)
      case _ if returnType =:= typeOf[Unit] =>
        (InvocationKind.FireAndForget, typeOf[Unit])
      case _ =>
        (InvocationKind.Awaitable, returnType)
    }
  }

  private def multiParamSchemaExpr(
    c: blackbox.Context
  )(methodName: String, params: List[(c.universe.TermName, c.universe.Type)]): c.Tree = {
    import c.universe._

    val expectedCount = params.length

    val paramEntries = params.map { case (name, tpe) =>
      val nameStr         = name.toString
      val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, tpe)
      val schemaInstance  = c.inferImplicitValue(golemSchemaType)
      if (schemaInstance.isEmpty) {
        c.abort(
          c.enclosingPosition,
          s"Unable to summon GolemSchema for parameter '$nameStr' of method $methodName with type $tpe.$schemaHint"
        )
      }
      q"($nameStr, $schemaInstance.asInstanceOf[_root_.golem.data.GolemSchema[Any]])"
    }

    q"""
      new _root_.golem.data.GolemSchema[Vector[Any]] {
        private val params = Array[(String, _root_.golem.data.GolemSchema[Any])](..$paramEntries)

        override val schema: _root_.golem.data.StructuredSchema = {
          val builder = List.newBuilder[_root_.golem.data.NamedElementSchema]
          var idx = 0
          while (idx < params.length) {
            val (paramName, codec) = params(idx)
            builder += _root_.golem.data.NamedElementSchema(paramName, codec.elementSchema)
            idx += 1
          }
          _root_.golem.data.StructuredSchema.Tuple(builder.result())
        }

        override def encode(value: Vector[Any]): Either[String, _root_.golem.data.StructuredValue] = {
          if (value.length != params.length)
            Left("Parameter count mismatch for method '" + $methodName + "'. Expected " + $expectedCount + ", found " + value.length)
          else {
            val builder = List.newBuilder[_root_.golem.data.NamedElementValue]
            var idx = 0
            var error: Option[String] = None
            while (idx < params.length && error.isEmpty) {
              val (paramName, codec) = params(idx)
              codec.encodeElement(value(idx)) match {
                case Left(err) =>
                  error = Some("Failed to encode parameter '" + paramName + "' in method '" + $methodName + "': " + err)
                case Right(elementValue) =>
                  builder += _root_.golem.data.NamedElementValue(paramName, elementValue)
              }
              idx += 1
            }
            error.fold[Either[String, _root_.golem.data.StructuredValue]](
              Right(_root_.golem.data.StructuredValue.Tuple(builder.result()))
            )(Left(_))
          }
        }

        override def decode(value: _root_.golem.data.StructuredValue): Either[String, Vector[Any]] =
          value match {
            case _root_.golem.data.StructuredValue.Tuple(elements) =>
              if (elements.length != params.length)
                Left("Structured element count mismatch for method '" + $methodName + "'. Expected " + $expectedCount + ", found " + elements.length)
              else {
                var idx = 0
                var error: Option[String] = None
                val buffer = Vector.newBuilder[Any]

                while (idx < params.length && error.isEmpty) {
                  val (paramName, codec) = params(idx)
                  val element = elements(idx)
                  if (element.name != paramName)
                    error = Some("Structured element name mismatch for method '" + $methodName + "'. Expected '" + paramName + "', found '" + element.name + "'")
                  else {
                    codec.decodeElement(element.value) match {
                      case Left(err) =>
                        error = Some("Failed to decode parameter '" + paramName + "' in method '" + $methodName + "': " + err)
                      case Right(decoded) =>
                        buffer += decoded
                    }
                  }
                  idx += 1
                }

                error.fold[Either[String, Vector[Any]]](Right(buffer.result()))(Left(_))
              }
            case other =>
              Left("Structured value mismatch for method '" + $methodName + "'. Expected tuple payload, found: " + other)
          }

        override def elementSchema: _root_.golem.data.ElementSchema =
          throw new UnsupportedOperationException("Multi-param schema cannot be used as a single element")

        override def encodeElement(value: Vector[Any]): Either[String, _root_.golem.data.ElementValue] =
          Left("Multi-param schema cannot be encoded as a single element")

        override def decodeElement(value: _root_.golem.data.ElementValue): Either[String, Vector[Any]] =
          Left("Multi-param schema cannot be decoded from a single element")
      }
    """
  }

  private sealed trait ParamAccessMode

  private sealed trait InvocationKind

  private object ParamAccessMode {
    case object NoArgs extends ParamAccessMode

    case object SingleArg extends ParamAccessMode

    case object MultiArgs extends ParamAccessMode
  }

  private object InvocationKind {
    case object Awaitable extends InvocationKind

    case object FireAndForget extends InvocationKind
  }
}
