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

import golem.runtime.{
  AgentMethod,
  AgentType,
  ConstructorType,
  InputRecordCodec,
  MethodInvocation,
  MethodMetadata,
  OutputCodec,
  ParamCodec
}
import golem.schema.{FromSchema, IntoSchema}
// Macro annotations live in a separate module; do not depend on them here.

import scala.quoted.*

object AgentClientMacro {
  private val schemaHint: String =
    "\nHint: IntoSchema/FromSchema are derived from zio.blocks.schema.Schema.\n" +
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

    val idParams = agentInputParams(traitRepr)
    val access   = idParams match {
      case Nil      => MethodParamAccess.NoArgs
      case _ :: Nil => MethodParamAccess.SingleArg
      case _        => MethodParamAccess.MultiArgs
    }
    val inputType = access match {
      case MethodParamAccess.NoArgs    => TypeRepr.of[Unit]
      case MethodParamAccess.SingleArg => idParams.head._2
      case MethodParamAccess.MultiArgs => TypeRepr.of[Vector[Any]]
    }

    val typeExpr =
      inputType.asType match {
        case '[input] =>
          val codecExpr = inputCodecExpr[input](access, "constructor", idParams)
          '{ ConstructorType[input]($codecExpr) }
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

  /**
   * The user-supplied `class Id(...)` parameters (name + type), Principal
   * params filtered out. These define the constructor input record's shape.
   */
  private def agentInputParams(using
    Quotes
  )(
    traitRepr: quotes.reflect.TypeRepr
  ): List[(String, quotes.reflect.TypeRepr)] = {
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
        classSym.primaryConstructor.paramSymss.flatten.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef => (sym.name, v.tpt.tpe)
              case other     => report.errorAndAbort(s"Unsupported parameter declaration in Id class: $other")
            }
        }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != "golem.Principal" }
    }
  }

  private def buildMethod[Trait: Type](using
    Quotes
  )(
    method: quotes.reflect.Symbol
  ): Expr[AgentMethod[Trait, ?, ?]] = {

    val functionName   = Expr(method.name)
    val methodNameExpr = Expr(method.name)

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
        val inputCodecExprV = inputCodecExpr[input](accessMode, s"method ${method.name}", parameters)

        outputType.asType match {
          case '[output] =>
            val outputCodecExprV = outputCodecExpr[output](s"method ${method.name}")

            '{
              val inputCodec  = $inputCodecExprV
              val outputCodec = $outputCodecExprV
              AgentMethod[Trait, input, output](
                metadata = MethodMetadata(
                  name = $methodNameExpr,
                  description = None,
                  prompt = None,
                  mode = None,
                  input = inputCodec.inputMetadata,
                  output = outputCodec.metadata
                ),
                functionName = $functionName,
                inputCodec = inputCodec,
                outputCodec = outputCodec,
                invocation = $invocationExpr
              )
            }
        }
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

  private def summonInto[A: Type](position: String)(using Quotes): Expr[IntoSchema[A]] =
    Expr.summon[IntoSchema[A]].getOrElse {
      import quotes.reflect.*
      report.errorAndAbort(s"Unable to summon IntoSchema for $position with type ${Type.show[A]}.$schemaHint")
    }

  private def summonFrom[A: Type](position: String)(using Quotes): Expr[FromSchema[A]] =
    Expr.summon[FromSchema[A]].getOrElse {
      import quotes.reflect.*
      report.errorAndAbort(s"Unable to summon FromSchema for $position with type ${Type.show[A]}.$schemaHint")
    }

  /**
   * Build the `InputRecordCodec[In]` for a constructor/method input from its
   * user-supplied parameters: `unit` (no args), `single` (one arg), or
   * `fromParams` (multiple args, encoded positionally as `Vector[Any]`).
   */
  private def inputCodecExpr[In: Type](using
    Quotes
  )(
    access: MethodParamAccess,
    context: String,
    params: List[(String, quotes.reflect.TypeRepr)]
  ): Expr[InputRecordCodec[In]] = {
    import quotes.reflect.*
    access match {
      case MethodParamAccess.NoArgs =>
        '{ InputRecordCodec.unit }.asExprOf[InputRecordCodec[In]]
      case MethodParamAccess.SingleArg =>
        val (name, tpe) = params.head
        tpe.asType match {
          case '[a] =>
            val into = summonInto[a](s"input of $context")
            val from = summonFrom[a](s"input of $context")
            '{ InputRecordCodec.single[a](${ Expr(name) })($into, $from) }.asExprOf[InputRecordCodec[In]]
        }
      case MethodParamAccess.MultiArgs =>
        val paramCodecs = paramCodecsExpr(context, params)
        '{ InputRecordCodec.fromParams($paramCodecs) }.asExprOf[InputRecordCodec[In]]
    }
  }

  private def paramCodecsExpr(using
    Quotes
  )(
    context: String,
    params: List[(String, quotes.reflect.TypeRepr)]
  ): Expr[List[ParamCodec]] = {
    val entries = params.map { case (name, tpe) =>
      tpe.asType match {
        case '[p] =>
          val into = summonInto[p](s"parameter '$name' of $context")
          val from = summonFrom[p](s"parameter '$name' of $context")
          '{
            ParamCodec(
              ${ Expr(name) },
              $into.asInstanceOf[IntoSchema[Any]],
              $from.asInstanceOf[FromSchema[Any]]
            )
          }
      }
    }
    Expr.ofList(entries)
  }

  /**
   * Build the `OutputCodec[Out]` for a method's return type: `unit` for `Unit`
   * (the host returns `none`), otherwise `single` carrying the value codec.
   */
  private def outputCodecExpr[Out: Type](using Quotes)(context: String): Expr[OutputCodec[Out]] = {
    import quotes.reflect.*
    if (TypeRepr.of[Out] =:= TypeRepr.of[Unit]) '{ OutputCodec.unit[Out] }
    else {
      val into = summonInto[Out](s"output of $context")
      val from = summonFrom[Out](s"output of $context")
      '{ OutputCodec.single[Out]($into, $from) }
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
