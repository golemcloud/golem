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

import golem.schema.{FromSchema, IntoSchema, TypedSchemaValue}
import golem.tool.*

import scala.concurrent.Future
import scala.quoted.*

object ToolMiddlewareMacro {

  inline def transparentHandle[Presented, Underlying, Surface, Impl <: Surface](
    underlying: RawToolUnderlying => Underlying
  ): MonomorphicToolMiddlewareHandle =
    ${ handleImpl[Presented, Presented, Underlying, Surface, Impl]('underlying, false) }

  inline def adapterHandle[Presented, Expected, Underlying, Surface, Impl <: Surface](
    underlying: RawToolUnderlying => Underlying
  ): MonomorphicToolMiddlewareHandle =
    ${ handleImpl[Presented, Expected, Underlying, Surface, Impl]('underlying, true) }

  inline def universalHandle[Impl <: UniversalToolMiddleware]: UniversalToolMiddlewareHandle =
    ${ universalHandleImpl[Impl] }

  private def handleImpl[
    Presented: Type,
    Expected: Type,
    Underlying: Type,
    Surface: Type,
    Impl: Type
  ](
    underlying: Expr[RawToolUnderlying => Underlying],
    adapter: Boolean
  )(using Quotes): Expr[MonomorphicToolMiddlewareHandle] = {
    val core = new ToolMacroCore
    new ToolMiddlewareAssembler(core).handleExpr[Presented, Expected, Underlying, Surface, Impl](underlying, adapter)
  }

  private def universalHandleImpl[Impl: Type](using Quotes): Expr[UniversalToolMiddlewareHandle] = {
    val core = new ToolMacroCore
    new ToolMiddlewareAssembler(core).universalHandleExpr[Impl]
  }
}

private[macros] final class ToolMiddlewareAssembler(val core: ToolMacroCore) {
  import core.q
  import q.reflect.*
  import ToolMacroExprs.given

  private final case class Leaf(path: List[String], method: core.MethodIR)

  def universalHandleExpr[Impl: Type]: Expr[UniversalToolMiddlewareHandle] = {
    val implRepr     = TypeRepr.of[Impl]
    val implSym      = implRepr.typeSymbol
    val universalTpe = TypeRepr.of[UniversalToolMiddleware]
    validateImplementation(universalTpe, implRepr, implSym)
    validateDirectParent(universalTpe, implSym)

    val constructor          = implSym.primaryConstructor
    val (name, aliases, doc) = core.universalToolMiddlewareMetadata(implSym)
    val instance             = Apply(Select(New(TypeTree.of[Impl]), constructor), Nil).asExprOf[Impl]

    '{
      UniversalToolMiddlewareHandle(
        ToolMiddlewareDescriptor(
          ${ Expr(name) },
          ${ Expr(aliases) },
          ${ Expr(doc) },
          ToolMiddlewareScope.Universal
        ),
        () => $instance.asInstanceOf[UniversalToolMiddleware]
      )
    }
  }

  def handleExpr[
    Presented: Type,
    Expected: Type,
    Underlying: Type,
    Surface: Type,
    Impl: Type
  ](
    underlyingFactory: Expr[RawToolUnderlying => Underlying],
    adapter: Boolean
  ): Expr[MonomorphicToolMiddlewareHandle] = {
    val presentedRepr  = TypeRepr.of[Presented]
    val expectedRepr   = TypeRepr.of[Expected]
    val underlyingRepr = TypeRepr.of[Underlying]
    val surfaceRepr    = TypeRepr.of[Surface]
    val implRepr       = TypeRepr.of[Impl]
    val implSym        = implRepr.typeSymbol

    validateImplementation(surfaceRepr, implRepr, implSym)
    validateGeneratedTypes(presentedRepr, expectedRepr, underlyingRepr, surfaceRepr, adapter)

    val constructor          = implSym.primaryConstructor
    val (name, aliases, doc) = core.toolMiddlewareMetadata(implSym)
    val presentedDescriptor  = new ToolDefinitionAssembler(core).descriptorExprOf[Presented]
    val expectedDescriptor   = new ToolDefinitionAssembler(core).descriptorExprOf[Expected]
    val leaves               = flatten(presentedRepr, Nil, Set.empty)
    val bindings             = Expr.ofList(leaves.map { leaf =>
      bindingExpr[Underlying, Surface](leaf, surfaceRepr, underlyingFactory)
    })

    val instance = Apply(Select(New(TypeTree.of[Impl]), constructor), Nil).asExpr

    '{
      MonomorphicToolMiddlewareHandle(
        descriptor = (ctx: ToolBuildCtx) =>
          for {
            presentedExtended <- $presentedDescriptor(ctx)
            expectedExtended  <- $expectedDescriptor(ctx)
            presentedWire     <- presentedExtended.tryToTool
            expectedWire      <- expectedExtended.tryToTool
          } yield ToolMiddlewareDescriptor(
            ${ Expr(name) },
            ${ Expr(aliases) },
            ${ Expr(doc) },
            ToolMiddlewareScope.Monomorphic(presentedWire, Some(expectedWire))
          ),
        presented = $presentedDescriptor,
        expected = $expectedDescriptor,
        newInstance = () => $instance,
        bindings = $bindings
      )
    }
  }

  private def validateImplementation(surface: TypeRepr, impl: TypeRepr, implSym: Symbol): Unit = {
    val pos = implSym.pos.getOrElse(Position.ofMacroExpansion)
    if (!impl.<:<(surface))
      report.errorAndAbort(
        s"tool middleware implementation ${implSym.fullName} must implement ${surface.show}",
        pos
      )
    if (implSym.flags.is(Flags.Abstract) || implSym.flags.is(Flags.Trait) || implSym.flags.is(Flags.Module))
      report.errorAndAbort(s"tool middleware implementation must be a concrete class: ${implSym.fullName}", pos)
    if (implSym.typeMembers.exists(_.isTypeParam) || impl.typeArgs.nonEmpty)
      report.errorAndAbort(s"tool middleware implementation must not have type parameters: ${implSym.fullName}", pos)

    val constructor = implSym.primaryConstructor
    if (
      constructor == Symbol.noSymbol || constructor.flags.is(Flags.Private) ||
      constructor.flags.is(Flags.Protected)
    )
      report.errorAndAbort(
        s"tool middleware implementation ${implSym.fullName} must have an accessible primary constructor",
        pos
      )
    val constructorParams = constructor.paramSymss.filter(_.forall(_.isTerm)).flatten
    if (constructorParams.nonEmpty)
      report.errorAndAbort(
        s"tool middleware implementation ${implSym.fullName} must have an empty primary constructor",
        pos
      )
  }

  private def validateDirectParent(parent: TypeRepr, implSym: Symbol): Unit = {
    val hasDirectParent = implSym.tree match {
      case classDef: ClassDef =>
        classDef.parents.exists {
          case typeTree: TypeTree => typeTree.tpe.dealias.typeSymbol == parent.typeSymbol
          case term: Term         => term.tpe.dealias.typeSymbol == parent.typeSymbol
          case _                  => false
        }
      case _ => false
    }
    if (!hasDirectParent)
      report.errorAndAbort(
        s"universal tool middleware implementation ${implSym.fullName} must directly extend ${parent.show}",
        implSym.pos.getOrElse(Position.ofMacroExpansion)
      )
  }

  private def validateGeneratedTypes(
    presented: TypeRepr,
    expected: TypeRepr,
    underlying: TypeRepr,
    surface: TypeRepr,
    adapter: Boolean
  ): Unit = {
    core.parseTool(presented)
    core.parseTool(expected)
    val expectedUnderlyingName = s"${expected.typeSymbol.fullName}Underlying"
    if (underlying.typeSymbol.fullName != expectedUnderlyingName)
      report.errorAndAbort(
        s"expected generated underlying type $expectedUnderlyingName, found ${underlying.show}"
      )
    val presentedPackage = presented.typeSymbol.owner.fullName
    val middlewareFqn    =
      if (presentedPackage.isEmpty) s"${presented.typeSymbol.name}Middleware"
      else s"$presentedPackage.${presented.typeSymbol.name}Middleware"
    val middlewareSymbol = presented.typeSymbol.owner
      .declaredType(s"${presented.typeSymbol.name}Middleware")
      .find(_.flags.is(Flags.Trait))
      .getOrElse(report.errorAndAbort(s"expected generated middleware surface $middlewareFqn, found ${surface.show}"))
    if (!adapter && (surface.typeSymbol != middlewareSymbol || surface.typeArgs.nonEmpty))
      report.errorAndAbort(s"expected generated middleware surface $middlewareFqn, found ${surface.show}")
    if (adapter) {
      val adapterSymbols = middlewareSymbol.companionModule.declaredType("Adapter")
      surface.dealias match {
        case AppliedType(constructor, List(argument))
            if adapterSymbols.contains(constructor.typeSymbol) && argument =:= underlying =>
          ()
        case _ =>
          report.errorAndAbort(
            s"expected generated middleware surface $middlewareFqn.Adapter[${underlying.show}], found ${surface.show}"
          )
      }
    }
  }

  private def flatten(toolRepr: TypeRepr, prefix: List[String], visited: Set[String]): List[Leaf] = {
    val ir  = core.parseTool(toolRepr)
    val fqn = ir.traitSym.fullName
    if (visited.contains(fqn))
      report.errorAndAbort(s"tool middleware projection contains a subtree cycle through $fqn")

    (ir.rootMethod.toList ++ ir.childMethods).flatMap { method =>
      val localPath = if (method.isRoot) Nil else List(method.commandName)
      val path      = prefix ++ localPath
      method.subtreeTrait match {
        case Some(child) => flatten(child, path, visited + fqn)
        case None        => List(Leaf(path, method))
      }
    }
  }

  private def bindingExpr[Underlying: Type, Surface: Type](
    leaf: Leaf,
    surfaceRepr: TypeRepr,
    underlyingFactory: Expr[RawToolUnderlying => Underlying]
  ): Expr[ToolMiddlewareMethodBinding] = {
    val candidates = surfaceRepr.typeSymbol.methodMember(leaf.method.methodName).filter(_.isDefDef)
    val methodSym  = candidates match {
      case method :: Nil => method
      case Nil           =>
        report.errorAndAbort(
          s"generated middleware surface ${surfaceRepr.show} is missing method ${leaf.method.methodName}"
        )
      case _ =>
        report.errorAndAbort(
          s"generated middleware method ${surfaceRepr.show}.${leaf.method.methodName} is ambiguous"
        )
    }
    val methodType = surfaceRepr.memberType(methodSym) match {
      case method: MethodType => method
      case other              =>
        report.errorAndAbort(
          s"generated middleware member ${surfaceRepr.show}.${leaf.method.methodName} is not a method: ${other.show}"
        )
    }
    if (methodType.paramTypes.isEmpty || !(methodType.paramTypes.head =:= TypeRepr.of[Underlying]))
      report.errorAndAbort(
        s"generated middleware method ${surfaceRepr.show}.${leaf.method.methodName} must take ${Type.show[Underlying]} as its first parameter"
      )

    val valueParamTypes   = methodType.paramTypes.drop(1)
    val valueParamSymbols = methodSym.paramSymss.filter(_.forall(_.isTerm)).flatten.drop(1)
    if (valueParamSymbols.length != valueParamTypes.length)
      report.errorAndAbort(
        s"generated middleware method ${surfaceRepr.show}.${leaf.method.methodName} has inconsistent parameter metadata"
      )
    val decoders = Expr.ofList(valueParamSymbols.zip(valueParamTypes).map { case (symbol, tpe) =>
      decoderExpr(symbol, tpe, leaf.method)
    })
    val expectsStdin = valueParamTypes.exists(_ =:= TypeRepr.of[ToolMiddlewareInputHandle])
    validateReturnType(methodType.resType, leaf.method)

    '{
      ToolMiddlewareMethodBinding(
        ${ Expr(leaf.method.methodName) },
        ${ Expr(leaf.path) },
        ${ Expr(expectsStdin) },
        (
          instance: Any,
          rawUnderlying: RawToolUnderlying,
          ctx: ToolMiddlewareInvocationContext
        ) => {
          val implementation = instance.asInstanceOf[Surface]
          val underlying     = $underlyingFactory(rawUnderlying)
          ToolMiddlewareInvokerRuntime.decodeArgs(ctx, $decoders) match {
            case Left(error) => Future.successful(Left(error))
            case Right(args) =>
              ${
                callAndEncode[Underlying, Surface](
                  leaf.method,
                  methodSym,
                  valueParamTypes,
                  'implementation,
                  'underlying,
                  'rawUnderlying,
                  'args
                )
              }
          }
        }
      )
    }
  }

  private def decoderExpr(
    symbol: Symbol,
    tpe: TypeRepr,
    method: core.MethodIR
  ): Expr[ToolMiddlewareParamDecoder] = {
    val pos = method.sym.pos.getOrElse(Position.ofMacroExpansion)
    if (core.isPrincipal(tpe)) '{ ToolMiddlewareParamDecoder.PrincipalParam }
    else if (tpe =:= TypeRepr.of[ToolMiddlewareInputHandle]) '{ ToolMiddlewareParamDecoder.StdinParam }
    else if (core.isStdout(tpe))
      report.errorAndAbort("generated middleware input methods must not contain stdout parameters", pos)
    else {
      val (canonicalName, countFlag) = core
        .toolMiddlewareFieldMetadata(symbol)
        .getOrElse(
          report.errorAndAbort(
            s"generated middleware parameter ${symbol.name} is missing canonical projection metadata",
            symbol.pos.getOrElse(pos)
          )
        )
      if (countFlag && !(tpe =:= TypeRepr.of[Int]))
        report.errorAndAbort(
          s"generated count-flag middleware parameter ${symbol.name} must have type Int",
          symbol.pos.getOrElse(pos)
        )
      tpe.asType match {
        case '[t] =>
          val from = Expr
            .summon[FromSchema[t]]
            .getOrElse(
              report.errorAndAbort(
                s"No implicit FromSchema available for middleware parameter type ${Type.show[t]}",
                pos
              )
            )
          if (countFlag)
            '{
              ToolMiddlewareParamDecoder.Field(
                ${ Expr(canonicalName) },
                field => ToolInvokerRuntime.countFlagDecoder(field.value)
              )
            }
          else
            '{
              ToolMiddlewareParamDecoder.Field(
                ${ Expr(canonicalName) },
                ToolMiddlewareInvokerRuntime.fieldDecoder[t]($from)
              )
            }
      }
    }
  }

  private def validateReturnType(actual: TypeRepr, method: core.MethodIR): Unit = {
    val pos    = method.sym.pos.getOrElse(Position.ofMacroExpansion)
    val either = core
      .futureArg(actual)
      .flatMap(core.eitherArgs)
      .getOrElse(
        report.errorAndAbort(
          s"generated middleware method ${method.methodName} must return Future[Either[ToolInvokeError[E], R]]",
          pos
        )
      )
    val actualError = either._1.dealias match {
      case AppliedType(constructor, List(error)) if constructor.typeSymbol.fullName == "golem.tool.ToolInvokeError" =>
        error
      case other =>
        report.errorAndAbort(
          s"generated middleware method ${method.methodName} must use ToolInvokeError, found ${other.show}",
          pos
        )
    }
    val expectedError = method.shape.kind match {
      case core.ReturnKind.EitherK(error, _) => error
      case _                                 => TypeRepr.of[Nothing]
    }
    if (!(actualError =:= expectedError))
      report.errorAndAbort(
        s"generated middleware method ${method.methodName} has error ${actualError.show}; expected ${expectedError.show}",
        pos
      )

    val hasStdout = method.params.exists(param => core.isStdout(param.tpe))
    val value     = method.shape.kind match {
      case core.ReturnKind.UnitK             => None
      case core.ReturnKind.Value(tpe)        => Some(tpe)
      case core.ReturnKind.EitherK(_, value) => value
    }
    val expectedSuccess = (value, hasStdout) match {
      case (None, false)      => TypeRepr.of[Unit]
      case (None, true)       => TypeRepr.of[ToolMiddlewareOutputHandle]
      case (Some(tpe), false) => tpe
      case (Some(tpe), true)  =>
        TypeRepr.of[Tuple2].appliedTo(List(tpe, TypeRepr.of[ToolMiddlewareOutputHandle]))
    }
    if (!(either._2 =:= expectedSuccess))
      report.errorAndAbort(
        s"generated middleware method ${method.methodName} has success ${either._2.show}; expected ${expectedSuccess.show}",
        pos
      )
  }

  private def callAndEncode[Underlying: Type, Surface: Type](
    method: core.MethodIR,
    methodSym: Symbol,
    paramTypes: List[TypeRepr],
    implementation: Expr[Surface],
    underlying: Expr[Underlying],
    rawUnderlying: Expr[RawToolUnderlying],
    args: Expr[Vector[Any]]
  ): Expr[Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]] = {
    val arguments = underlying.asTerm :: paramTypes.zipWithIndex.map { case (tpe, index) =>
      tpe.asType match {
        case '[t] => '{ $args(${ Expr(index) }).asInstanceOf[t] }.asTerm
      }
    }
    val call      = Apply(Select(implementation.asTerm, methodSym), arguments)
    val hasStdout = method.params.exists(param => core.isStdout(param.tpe))

    method.shape.kind match {
      case core.ReturnKind.EitherK(errorType, valueType) =>
        errorType.asType match {
          case '[e] =>
            val schema = new ToolErrorSchemaAssembler(core).deriveExpr[e]
            valueType match {
              case None if hasStdout =>
                '{
                  val errorSchema = $schema
                  ${ call.asExprOf[Future[Either[ToolInvokeError[e], ToolMiddlewareOutputHandle]]] }.map {
                    case Left(error)   => Left(ToolMiddlewareInvokerRuntime.encodeError(error, errorSchema))
                    case Right(stdout) => ToolMiddlewareInvokerRuntime.encodeStdout(stdout, $rawUnderlying)
                  }(ToolInvokerRuntime.executionContext)
                }
              case None =>
                '{
                  val errorSchema = $schema
                  ${ call.asExprOf[Future[Either[ToolInvokeError[e], Unit]]] }.map {
                    case Left(error) => Left(ToolMiddlewareInvokerRuntime.encodeError(error, errorSchema))
                    case Right(_)    => ToolMiddlewareInvokerRuntime.encodeUnit
                  }(ToolInvokerRuntime.executionContext)
                }
              case Some(value) =>
                value.asType match {
                  case '[a] =>
                    val into = summonIntoSchema[a](method)
                    if (hasStdout)
                      '{
                        val errorSchema = $schema
                        ${
                          call.asExprOf[
                            Future[Either[ToolInvokeError[e], (a, ToolMiddlewareOutputHandle)]]
                          ]
                        }.map {
                          case Left(error)             => Left(ToolMiddlewareInvokerRuntime.encodeError(error, errorSchema))
                          case Right((result, stdout)) =>
                            ToolMiddlewareInvokerRuntime.encodeValueStdout(result, stdout, $into, $rawUnderlying)
                        }(ToolInvokerRuntime.executionContext)
                      }
                    else
                      '{
                        val errorSchema = $schema
                        ${ call.asExprOf[Future[Either[ToolInvokeError[e], a]]] }.map {
                          case Left(error)  => Left(ToolMiddlewareInvokerRuntime.encodeError(error, errorSchema))
                          case Right(value) => ToolMiddlewareInvokerRuntime.encodeValue(value, $into)
                        }(ToolInvokerRuntime.executionContext)
                      }
                }
            }
        }

      case core.ReturnKind.UnitK if hasStdout =>
        '{
          ${ call.asExprOf[Future[Either[ToolInvokeError[Nothing], ToolMiddlewareOutputHandle]]] }.map {
            case Left(error)   => Left(ToolMiddlewareInvokerRuntime.encodeInfallibleError(error))
            case Right(stdout) => ToolMiddlewareInvokerRuntime.encodeStdout(stdout, $rawUnderlying)
          }(ToolInvokerRuntime.executionContext)
        }
      case core.ReturnKind.UnitK =>
        '{
          ${ call.asExprOf[Future[Either[ToolInvokeError[Nothing], Unit]]] }.map {
            case Left(error) => Left(ToolMiddlewareInvokerRuntime.encodeInfallibleError(error))
            case Right(_)    => ToolMiddlewareInvokerRuntime.encodeUnit
          }(ToolInvokerRuntime.executionContext)
        }
      case core.ReturnKind.Value(value) =>
        value.asType match {
          case '[a] =>
            val into = summonIntoSchema[a](method)
            if (hasStdout)
              '{
                ${
                  call.asExprOf[
                    Future[Either[ToolInvokeError[Nothing], (a, ToolMiddlewareOutputHandle)]]
                  ]
                }.map {
                  case Left(error)             => Left(ToolMiddlewareInvokerRuntime.encodeInfallibleError(error))
                  case Right((result, stdout)) =>
                    ToolMiddlewareInvokerRuntime.encodeValueStdout(result, stdout, $into, $rawUnderlying)
                }(ToolInvokerRuntime.executionContext)
              }
            else
              '{
                ${ call.asExprOf[Future[Either[ToolInvokeError[Nothing], a]]] }.map {
                  case Left(error)  => Left(ToolMiddlewareInvokerRuntime.encodeInfallibleError(error))
                  case Right(value) => ToolMiddlewareInvokerRuntime.encodeValue(value, $into)
                }(ToolInvokerRuntime.executionContext)
              }
        }
    }
  }

  private def summonIntoSchema[A: Type](method: core.MethodIR): Expr[IntoSchema[A]] =
    Expr
      .summon[IntoSchema[A]]
      .getOrElse(
        report.errorAndAbort(
          s"No implicit IntoSchema available for middleware result type ${Type.show[A]}",
          method.sym.pos.getOrElse(Position.ofMacroExpansion)
        )
      )
}
