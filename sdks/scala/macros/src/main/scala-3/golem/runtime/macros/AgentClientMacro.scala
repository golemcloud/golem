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

import golem.data.{ElementSchema, GolemSchema, NamedElementSchema, StructuredSchema}
import golem.runtime.{AgentMethod, AgentType, ConstructorType, MethodInvocation, MethodMetadata}
// Macro annotations live in a separate module; do not depend on them here.

import scala.quoted.*

object AgentClientMacro {
  private val schemaHint: String =
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  transparent inline def agentType[Trait]: AgentType[Trait, ?] =
    ${ agentTypeImpl[Trait] }

  private def agentTypeImpl[Trait: Type](using Quotes): Expr[AgentType[Trait, ?]] = {
    import quotes.reflect.*

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"Agent client target must be a trait, found: ${traitSymbol.fullName}")

    val (constructorTpe, constructorTypeExpr) = buildConstructorType(traitRepr)
    val agentTypeName                         = agentTypeNameOrDefault(traitSymbol)

    val methods = traitSymbol.methodMembers.collect {
      case method if method.flags.is(Flags.Deferred) && method.isDefDef && method.name != "new" =>
        buildMethod[Trait](method)
    }

    val traitNameExpr = Expr(agentTypeName)
    val methodsExpr   = Expr.ofList(methods)

    constructorTpe.asType match {
      case '[ctor] =>
        val traitClassNameExpr = Expr(traitSymbol.fullName)
        '{
          AgentType[Trait, ctor](
            traitClassName = $traitClassNameExpr,
            typeName = $traitNameExpr,
            constructor = $constructorTypeExpr.asInstanceOf[ConstructorType[ctor]],
            methods = $methodsExpr
          )
        }
    }
  }

  private def buildConstructorType(using
    Quotes
  )(
    traitRepr: quotes.reflect.TypeRepr
  ): (quotes.reflect.TypeRepr, Expr[ConstructorType[?]]) = {
    import quotes.reflect.*

    val inputType  = agentInputType(traitRepr)
    val accessMode =
      if inputType =:= TypeRepr.of[Unit] then MethodParamAccess.NoArgs
      else MethodParamAccess.SingleArg
    val parameters =
      if accessMode == MethodParamAccess.SingleArg then List(("args", inputType)) else Nil

    val typeExpr =
      inputType.asType match {
        case '[input] =>
          val schemaExpr: Expr[GolemSchema[input]] = accessMode match {
            case MethodParamAccess.MultiArgs =>
              multiParamSchemaExpr("new", parameters).asExprOf[GolemSchema[input]]
            case _ =>
              summonSchema[input]("new", "input")
          }
          '{ ConstructorType[input]($schemaExpr) }
      }

    (inputType, typeExpr)
  }

  private def agentTypeNameOrDefault(using Quotes)(traitSymbol: quotes.reflect.Symbol): String = {
    import quotes.reflect.*
    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name

    def extractTypeName(args: List[Term]): Option[String] =
      args.collectFirst {
        case Literal(StringConstant(value))                   => value
        case NamedArg("typeName", Literal(StringConstant(v))) => v
      }

    val annArgsOpt =
      traitSymbol.annotations.collectFirst {
        case Apply(Select(New(tpt), _), args)
            if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
          args
      }

    annArgsOpt match {
      case None =>
        // Keep agent clients consistent with AgentNameMacro: require an explicit marker annotation.
        report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${traitSymbol.fullName}")
      case Some(args) =>
        extractTypeName(args) match {
          case Some(value) if value.trim.nonEmpty => validateTypeName(value)
          case _                                  => defaultTypeNameFromTrait(traitSymbol)
        }
    }
  }

  private def validateTypeName(value: String): String =
    value

  private def agentInputType(using
    Quotes
  )(
    traitRepr: quotes.reflect.TypeRepr
  ): quotes.reflect.TypeRepr = {
    import quotes.reflect.*
    val typeSymbol = traitRepr.typeSymbol

    val idFQN = "golem.runtime.annotations.id"

    def hasIdAnnotation(sym: Symbol): Boolean =
      sym.annotations.exists {
        case Apply(Select(New(tpt), _), _) => tpt.tpe.dealias.typeSymbol.fullName == idFQN
        case _                             => false
      }

    val constructorClass = typeSymbol.declarations.find { sym =>
      sym.isClassDef && hasIdAnnotation(sym)
    }.orElse {
      typeSymbol.declarations.find { sym =>
        sym.isClassDef && sym.name == "Id"
      }
    }

    constructorClass match {
      case None =>
        report.errorAndAbort(
          s"Agent trait ${typeSymbol.name} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
        )
      case Some(classSym) =>
        val primaryCtor = classSym.primaryConstructor
        val params      = primaryCtor.paramSymss.flatten.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef => v.tpt.tpe
              case _         => TypeRepr.of[Nothing]
            }
        }
        params match {
          case Nil      => TypeRepr.of[Unit]
          case p :: Nil => p
          case ps       =>
            val tupleClass = Symbol.requiredClass(s"scala.Tuple${ps.length}")
            tupleClass.typeRef.appliedTo(ps)
        }
    }
  }

  private def buildMethod[Trait: Type](using
    Quotes
  )(
    method: quotes.reflect.Symbol
  ): Expr[AgentMethod[Trait, ?, ?]] = {

    val metadataExpr = methodMetadata(method)
    val functionName = Expr(method.name)

    val parameters                   = extractParameters(method)
    val accessMode                   = methodAccess(parameters)
    val inputType                    = inputTypeFor(accessMode, parameters)
    val (invocationKind, outputType) = methodInvocationInfo(method)
    val invocationExpr               = invocationKind match {
      case InvocationKind.Awaitable     => '{ MethodInvocation.Awaitable }
      case InvocationKind.FireAndForget => '{ MethodInvocation.FireAndForget }
    }

    inputType.asType match {
      case '[input] =>
        val inputSchemaExpr: Expr[GolemSchema[input]] =
          accessMode match {
            case MethodParamAccess.MultiArgs =>
              multiParamSchemaExpr(method.name, parameters).asExprOf[GolemSchema[input]]
            case _ =>
              summonSchema[input](method.name, "input")
          }

        outputType.asType match {
          case '[output] =>
            val outputSchemaExpr: Expr[GolemSchema[output]] =
              invocationKind match {
                case InvocationKind.Awaitable =>
                  summonSchema[output](method.name, "output")
                case InvocationKind.FireAndForget =>
                  summonSchema[output](method.name, "output")
              }

            '{
              AgentMethod[Trait, input, output](
                metadata = $metadataExpr,
                functionName = $functionName,
                inputSchema = $inputSchemaExpr,
                outputSchema = $outputSchemaExpr,
                invocation = $invocationExpr
              )
            }
        }
    }
  }

  private def methodMetadata(using Quotes)(method: quotes.reflect.Symbol): Expr[MethodMetadata] = {

    val methodName   = Expr(method.name)
    val inputSchema  = methodInputSchema(method)
    val outputSchema = methodOutputSchema(method)

    '{
      MethodMetadata(
        name = $methodName,
        description = None,
        prompt = None,
        mode = None,
        input = $inputSchema,
        output = $outputSchema
      )
    }
  }

  private def methodInputSchema(using Quotes)(method: quotes.reflect.Symbol): Expr[StructuredSchema] = {

    val params = extractParameters(method)

    if params.isEmpty then '{ StructuredSchema.Tuple(Nil) }
    else if params.length == 1 then {
      val (_, paramType) = params.head
      structuredSchemaExpr(paramType)
    } else {
      val elements = params.map { case (name, tpe) =>
        val schemaExpr = elementSchemaExpr(name, tpe)
        '{ NamedElementSchema(${ Expr(name) }, $schemaExpr) }
      }
      val listExpr = Expr.ofList(elements)
      '{ StructuredSchema.Tuple($listExpr) }
    }
  }

  private def elementSchemaExpr(using
    Quotes
  )(@scala.annotation.unused paramName: String, tpe: quotes.reflect.TypeRepr): Expr[ElementSchema] = {
    import quotes.reflect.*

    tpe.asType match {
      case '[t] =>
        Expr.summon[GolemSchema[t]] match {
          case Some(schemaExpr) =>
            '{ $schemaExpr.elementSchema }
          case None =>
            report.errorAndAbort(s"No implicit GolemSchema available for type ${Type.show[t]}.$schemaHint")
        }
    }
  }

  private def methodOutputSchema(using Quotes)(method: quotes.reflect.Symbol): Expr[StructuredSchema] = {
    import quotes.reflect.*
    method.tree match {
      case d: DefDef =>
        val outputType = unwrapAsyncType(d.returnTpt.tpe)
        structuredSchemaExpr(outputType)
      case other =>
        report.errorAndAbort(s"Unable to read return type for ${method.name}: $other")
    }
  }

  private def structuredSchemaExpr(using Quotes)(tpe: quotes.reflect.TypeRepr): Expr[StructuredSchema] = {
    import quotes.reflect.*
    tpe.asType match {
      case '[t] =>
        Expr.summon[GolemSchema[t]] match {
          case Some(schemaExpr) =>
            '{ $schemaExpr.schema }
          case None =>
            report.errorAndAbort(s"No implicit GolemSchema available for type ${Type.show[t]}.$schemaHint")
        }
    }
  }

  private def unwrapAsyncType(using Quotes)(tpe: quotes.reflect.TypeRepr): quotes.reflect.TypeRepr = {
    import quotes.reflect.*
    tpe match {
      case AppliedType(constructor, args) if isAsyncReturn(constructor) && args.nonEmpty =>
        args.head
      case other =>
        other
    }
  }

  private def isAsyncReturn(using Quotes)(constructor: quotes.reflect.TypeRepr): Boolean = {
    val name = constructor.typeSymbol.fullName
    name == "scala.concurrent.Future" || name == "scala.scalajs.js.Promise"
  }

  private def extractParameters(using
    Quotes
  )(method: quotes.reflect.Symbol): List[(String, quotes.reflect.TypeRepr)] = {
    import quotes.reflect.*
    method.paramSymss.collectFirst {
      case params if params.forall(_.isTerm) =>
        params.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef => (sym.name, v.tpt.tpe)
              case other     => report.errorAndAbort(s"Unsupported parameter declaration in ${method.name}: $other")
            }
        }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != "golem.Principal" }
    }.getOrElse(Nil)
  }

  private def methodAccess(using Quotes)(parameters: List[(String, quotes.reflect.TypeRepr)]): MethodParamAccess =
    parameters match {
      case Nil      => MethodParamAccess.NoArgs
      case _ :: Nil => MethodParamAccess.SingleArg
      case _        => MethodParamAccess.MultiArgs
    }

  private def inputTypeFor(using
    Quotes
  )(
    access: MethodParamAccess,
    parameters: List[(String, quotes.reflect.TypeRepr)]
  ): quotes.reflect.TypeRepr =
    access match {
      case MethodParamAccess.NoArgs    => quotes.reflect.TypeRepr.of[Unit]
      case MethodParamAccess.SingleArg => parameters.head._2
      case MethodParamAccess.MultiArgs => quotes.reflect.TypeRepr.of[Vector[Any]]
    }

  private def methodInvocationInfo(using
    Quotes
  )(
    method: quotes.reflect.Symbol
  ): (InvocationKind, quotes.reflect.TypeRepr) = {
    import quotes.reflect.*
    method.tree match {
      case d: DefDef =>
        val returnType = d.returnTpt.tpe
        returnType match {
          case AppliedType(constructor, args) if isAsyncReturn(constructor) && args.nonEmpty =>
            (InvocationKind.Awaitable, args.head)
          case _ if returnType =:= TypeRepr.of[Unit] =>
            (InvocationKind.FireAndForget, TypeRepr.of[Unit])
          case _ =>
            (InvocationKind.Awaitable, returnType)
        }
      case other =>
        report.errorAndAbort(s"Unable to read return type for ${method.name}: $other")
    }
  }

  private def summonSchema[A: Type](methodName: String, position: String)(using Quotes): Expr[GolemSchema[A]] =
    Expr.summon[GolemSchema[A]].getOrElse {
      import quotes.reflect.*
      report.errorAndAbort(
        s"Unable to summon GolemSchema for $position of method $methodName with type ${Type.show[A]}.$schemaHint"
      )
    }

  private def multiParamSchemaExpr(using
    Quotes
  )(
    methodName: String,
    params: List[(String, quotes.reflect.TypeRepr)]
  ): Expr[GolemSchema[Vector[Any]]] = {

    val methodNameExpr    = Expr(methodName)
    val expectedCountExpr = Expr(params.length)

    val paramEntries: Seq[Expr[(String, GolemSchema[Any])]] =
      params.map { case (name, tpe) =>
        tpe.asType match {
          case '[p] =>
            val codecExpr = summonSchema[p](methodName, s"parameter '$name'")
            '{ (${ Expr(name) }, $codecExpr.asInstanceOf[GolemSchema[Any]]) }
        }
      }

    val paramsArrayExpr =
      '{ Array[(String, GolemSchema[Any])](${ Varargs(paramEntries) }*) }

    '{
      new GolemSchema[Vector[Any]] {
        private val params = $paramsArrayExpr

        override val schema: _root_.golem.data.StructuredSchema = {
          val builder = List.newBuilder[_root_.golem.data.NamedElementSchema]
          var idx     = 0
          while (idx < params.length) {
            val (paramName, codec) = params(idx)
            builder += _root_.golem.data.NamedElementSchema(paramName, codec.elementSchema)
            idx += 1
          }
          _root_.golem.data.StructuredSchema.Tuple(builder.result())
        }

        override def encode(value: Vector[Any]): Either[String, _root_.golem.data.StructuredValue] =
          if (value.length != params.length)
            Left(
              s"Parameter count mismatch for method '${$methodNameExpr}'. Expected ${$expectedCountExpr}, found ${value.length}"
            )
          else {
            val builder = List.newBuilder[_root_.golem.data.NamedElementValue]
            var idx     = 0
            while (idx < params.length) {
              val (paramName, codec) = params(idx)
              codec.encodeElement(value(idx)) match {
                case Left(err) =>
                  return Left(s"Failed to encode parameter '$paramName' in method '${$methodNameExpr}': $err")
                case Right(elementValue) =>
                  builder += _root_.golem.data.NamedElementValue(paramName, elementValue)
              }
              idx += 1
            }
            Right(_root_.golem.data.StructuredValue.Tuple(builder.result()))
          }

        override def decode(
          value: _root_.golem.data.StructuredValue
        ): Either[String, Vector[Any]] =
          value match {
            case _root_.golem.data.StructuredValue.Tuple(elements) =>
              if (elements.length != params.length)
                Left(
                  s"Structured element count mismatch for method '${$methodNameExpr}'. Expected ${$expectedCountExpr}, found ${elements.length}"
                )
              else {
                var idx     = 0
                var failure = Option.empty[String]
                val buffer  = Vector.newBuilder[Any]

                while (idx < params.length && failure.isEmpty) {
                  val (paramName, codec) = params(idx)
                  val element            = elements(idx)
                  if (element.name != paramName)
                    failure = Some(
                      s"Structured element name mismatch for method '${$methodNameExpr}'. Expected '$paramName', found '${element.name}'"
                    )
                  else {
                    codec.decodeElement(element.value) match {
                      case Left(err) =>
                        failure = Some(s"Failed to decode parameter '$paramName' in method '${$methodNameExpr}': $err")
                      case Right(decoded) =>
                        buffer += decoded
                    }
                  }
                  idx += 1
                }

                failure.fold[Either[String, Vector[Any]]](Right(buffer.result()))(Left(_))
              }
            case other =>
              Left(s"Structured value mismatch for method '${$methodNameExpr}'. Expected tuple payload, found: $other")
          }

        override def elementSchema: _root_.golem.data.ElementSchema =
          throw new UnsupportedOperationException("Multi-param schema cannot be used as a single element")

        override def encodeElement(value: Vector[Any]): Either[String, _root_.golem.data.ElementValue] =
          Left("Multi-param schema cannot be encoded as a single element")

        override def decodeElement(value: _root_.golem.data.ElementValue): Either[String, Vector[Any]] =
          Left("Multi-param schema cannot be decoded from a single element")
      }
    }
  }

  private enum MethodParamAccess {
    case NoArgs
    case SingleArg
    case MultiArgs
  }

  private enum InvocationKind {
    case Awaitable
    case FireAndForget
  }
}
