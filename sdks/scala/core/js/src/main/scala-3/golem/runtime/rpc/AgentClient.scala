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

package golem.runtime.rpc

import golem.runtime.macros.AgentClientMacro
import golem.runtime.{AgentMethod, AgentType}
import golem.Uuid

import scala.collection.immutable.Vector
import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js

object AgentClient {
  transparent inline def agentType[Trait]: AgentType[Trait, ?] =
    AgentClientMacro.agentType[Trait]

  /**
   * Typed agent-type accessor (no user-land casts).
   *
   * This exists because Scala.js cannot safely cast a plain JS object to a
   * Scala trait at runtime. When you need to operate at the "agent type +
   * resolved client" level (e.g. in internal wiring), use this API to keep
   * examples cast-free. Constructor type is always Unit (temporary).
   */
  transparent inline def agentTypeWithCtor[Trait, Constructor]: AgentType[Trait, Constructor] =
    ${ AgentTypeMacro.agentTypeWithCtorImpl[Trait, Constructor] }

  transparent inline def connect[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor
  ): Trait =
    AgentClientInlineMacros.connect[Trait, Constructor](agentType, constructorArgs)

  transparent inline def connectPhantom[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor,
    phantom: Uuid
  ): Trait =
    AgentClientInlineMacros.connectPhantom[Trait, Constructor](agentType, constructorArgs, phantom)

  transparent inline def bind[Trait](
    resolved: AgentClientRuntime.ResolvedAgent[Trait]
  ): Trait =
    AgentClientRuntime.TestHooks.bindOverride(resolved).getOrElse {
      AgentClientInlineMacros.bind[Trait](resolved)
    }
}

private object AgentTypeMacro {
  import scala.quoted.*

  def agentTypeWithCtorImpl[Trait: Type, Constructor: Type](using Quotes): Expr[AgentType[Trait, Constructor]] = {
    import quotes.reflect.*

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"Agent client target must be a trait, found: ${traitSymbol.fullName}")

    '{ AgentClientMacro.agentType[Trait].asInstanceOf[AgentType[Trait, Constructor]] }
  }
}

private object AgentClientInlineMacros {
  import scala.quoted.*

  transparent inline def connect[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor
  ): Trait =
    ${ connectImpl[Trait, Constructor]('agentType, 'constructorArgs) }

  transparent inline def connectPhantom[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor,
    phantom: Uuid
  ): Trait =
    ${ connectPhantomImpl[Trait, Constructor]('agentType, 'constructorArgs, 'phantom) }

  transparent inline def bind[Trait](
    resolved: AgentClientRuntime.ResolvedAgent[Trait]
  ): Trait =
    ${ stubImpl[Trait]('resolved) }

  private def connectImpl[Trait: Type, Constructor: Type](
    agentTypeExpr: Expr[AgentType[Trait, Constructor]],
    constructorExpr: Expr[Constructor]
  )(using Quotes): Expr[Trait] =
    '{
      AgentClientRuntime.resolve[Trait, Constructor]($agentTypeExpr, $constructorExpr) match {
        case Left(err) =>
          throw js.JavaScriptException(err)
        case Right(resolved) =>
          ${ stubImpl[Trait]('resolved) }
      }
    }

  private def connectPhantomImpl[Trait: Type, Constructor: Type](
    agentTypeExpr: Expr[AgentType[Trait, Constructor]],
    constructorExpr: Expr[Constructor],
    phantomExpr: Expr[Uuid]
  )(using Quotes): Expr[Trait] =
    '{
      AgentClientRuntime.resolveWithPhantom[Trait, Constructor](
        $agentTypeExpr,
        $constructorExpr,
        phantom = Some($phantomExpr)
      ) match {
        case Left(err) =>
          throw js.JavaScriptException(err)
        case Right(resolved) =>
          ${ stubImpl[Trait]('resolved) }
      }
    }

  private def stubImpl[Trait: Type](
    resolvedExpr: Expr[AgentClientRuntime.ResolvedAgent[Trait]]
  )(using Quotes): Expr[Trait] = {
    import quotes.reflect.*

    case class MethodData(
      method: Symbol,
      params: List[(String, TypeRepr)],
      nonPrincipalParams: List[(String, TypeRepr)],
      accessMode: MethodParamAccess,
      inputType: TypeRepr,
      outputType: TypeRepr,
      returnType: TypeRepr,
      invocation: InvocationKind
    )

    def isPrincipalType(tpe: TypeRepr): Boolean =
      tpe.dealias.typeSymbol.fullName == "golem.Principal"

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"Agent client target must be a trait, found: ${traitSymbol.fullName}")

    val pendingMethods = traitSymbol.methodMembers.collect {
      case method if method.flags.is(Flags.Deferred) && method.isDefDef && method.name != "new" =>
        val params                                   = extractParameters(method)
        val nonPrincipalParams                       = params.filter { case (_, tpe) => !isPrincipalType(tpe) }
        val accessMode                               = methodAccess(nonPrincipalParams)
        val inputType                                = inputTypeFor(accessMode, nonPrincipalParams)
        val (invocationKind, outputType, returnType) = methodInvocationInfo(method)

        MethodData(
          method = method,
          params = params,
          nonPrincipalParams = nonPrincipalParams,
          accessMode = accessMode,
          inputType = inputType,
          outputType = outputType,
          returnType = returnType,
          invocation = invocationKind
        )
    }

    val resolvedSym = Symbol.newVal(
      Symbol.spliceOwner,
      "$resolvedAgent",
      TypeRepr.of[AgentClientRuntime.ResolvedAgent[Trait]],
      Flags.EmptyFlags,
      Symbol.noSymbol
    )

    val resolvedVal = ValDef(resolvedSym, Some(resolvedExpr.asTerm))
    val resolvedRef = Ref(resolvedSym).asExprOf[AgentClientRuntime.ResolvedAgent[Trait]]

    def buildInputValueExpr(accessMode: MethodParamAccess, inputType: TypeRepr, params: List[Expr[Any]]): Expr[Any] =
      inputType.asType match {
        case '[input] =>
          accessMode match {
            case MethodParamAccess.NoArgs =>
              '{ ().asInstanceOf[input] }
            case MethodParamAccess.SingleArg =>
              params.headOption
                .getOrElse(report.errorAndAbort("Single argument access mode requires exactly one argument"))
                .asExprOf[input]
            case MethodParamAccess.MultiArgs =>
              val elements = params.map(_.asExprOf[Any])
              '{ Vector[Any](${ Varargs(elements) }*) }.asExprOf[input]
          }
      }

    def findMethod[In: Type, Out: Type](methodName: String): Expr[AgentMethod[Trait, In, Out]] = {
      val methodNameExpr = Expr(methodName)
      '{
        $resolvedRef.agentType.methods.collectFirst {
          case m if m.metadata.name == $methodNameExpr =>
            m.asInstanceOf[AgentMethod[Trait, In, Out]]
        }
          .getOrElse(throw new IllegalStateException(s"Method definition for ${$methodNameExpr} not found"))
      }
    }

    def buildMethodBody(methodData: MethodData, paramExprs: List[Expr[Any]]): Expr[Any] = {
      val methodNameExpr: Expr[String] = Expr(methodData.method.name)

      val nonPrincipalParamExprs = paramExprs
        .zip(methodData.params)
        .collect { case (expr, (_, tpe)) if !isPrincipalType(tpe) => expr }

      methodData.inputType.asType match {
        case '[input] =>
          methodData.outputType.asType match {
            case '[output] =>
              val inputValueExpr =
                buildInputValueExpr(methodData.accessMode, methodData.inputType, nonPrincipalParamExprs).asExprOf[input]
              val methodExpr =
                findMethod[input, output](methodData.method.name)

              methodData.invocation match {
                case InvocationKind.Awaitable =>
                  '{

                    $resolvedRef.call[input, output](
                      $methodExpr,
                      $inputValueExpr
                    )
                  }
                case InvocationKind.FireAndForget =>
                  if !(methodData.outputType =:= TypeRepr.of[Unit]) then
                    report.errorAndAbort(s"Fire-and-forget method ${methodData.method.name} must return Unit")
                  val triggerMethodExpr =
                    methodExpr.asExprOf[AgentMethod[Trait, input, Unit]]
                  '{
                    import scala.concurrent.ExecutionContext.Implicits.global
                    $resolvedRef
                      .trigger[input](
                        $triggerMethodExpr,
                        $inputValueExpr
                      )
                      .failed
                      .foreach(err =>
                        js.Dynamic.global.console.error(
                          s"RPC trigger ${$methodNameExpr} failed",
                          err.asInstanceOf[js.Any]
                        )
                      )
                    ()
                  }
                case InvocationKind.UnsupportedSync =>
                  val msg =
                    s"Agent client method ${methodData.method.name} must return scala.concurrent.Future[...] or Unit when invoked via RPC."
                  '{ throw new IllegalStateException(${ Expr(msg) }) }
              }
          }
      }
    }

    // Generate an anonymous class that extends Trait with concrete method
    // implementations delegating to the resolved agent's RPC call.
    // This ensures that fullLinkJS name minification is applied consistently
    // to both the method definitions and their call sites.
    val clsSym = Symbol.newClass(
      Symbol.spliceOwner,
      "$proxy",
      parents = List(TypeRepr.of[Object], traitRepr),
      decls = cls =>
        pendingMethods.map { data =>
          Symbol.newMethod(
            cls,
            data.method.name,
            traitRepr.memberType(data.method),
            Flags.Override,
            Symbol.noSymbol
          )
        },
      selfType = None
    )

    val methodDefs: List[DefDef] = pendingMethods.map { data =>
      val methodSym = clsSym.declaredMethod(data.method.name).head
      DefDef(
        methodSym,
        { argss =>
          val paramExprs = argss.flatten.collect { case t: Term => t.asExprOf[Any] }.toList
          Some(buildMethodBody(data, paramExprs).asTerm.changeOwner(methodSym))
        }
      )
    }

    val classDef = ClassDef(
      clsSym,
      List(
        Apply(
          Select(New(TypeTree.of[Object]), TypeRepr.of[Object].typeSymbol.primaryConstructor),
          Nil
        ),
        TypeTree.of[Trait]
      ),
      body = methodDefs
    )

    val newInstance = Typed(
      Apply(Select(New(TypeIdent(clsSym)), clsSym.primaryConstructor), Nil),
      TypeTree.of[Trait]
    )

    Block(
      List(resolvedVal, classDef),
      newInstance
    ).asExprOf[Trait]
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
        }
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
      case MethodParamAccess.SingleArg => parameters.headOption.fold(quotes.reflect.TypeRepr.of[Unit])(_._2)
      case MethodParamAccess.MultiArgs => quotes.reflect.TypeRepr.of[Vector[Any]]
    }

  private def methodInvocationInfo(using
    Quotes
  )(
    method: quotes.reflect.Symbol
  ): (InvocationKind, quotes.reflect.TypeRepr, quotes.reflect.TypeRepr) = {
    import quotes.reflect.*
    method.tree match {
      case d: DefDef =>
        val returnType = d.returnTpt.tpe
        returnType match {
          case AppliedType(constructor, args) if isAsyncReturn(constructor) && args.nonEmpty =>
            (InvocationKind.Awaitable, args.head, returnType)
          case _ =>
            if returnType =:= TypeRepr.of[Unit] then
              (InvocationKind.FireAndForget, TypeRepr.of[Unit], TypeRepr.of[Unit])
            else {
              (InvocationKind.UnsupportedSync, returnType, returnType)
            }
        }
      case other =>
        report.errorAndAbort(s"Unable to read return type for ${method.name}: $other")
    }
  }

  private def isAsyncReturn(using Quotes)(constructor: quotes.reflect.TypeRepr): Boolean = {
    val name = constructor.typeSymbol.fullName
    name == "scala.concurrent.Future" || name == "scala.scalajs.js.Promise"
  }

  private enum MethodParamAccess {
    case NoArgs
    case SingleArg
    case MultiArgs
  }

  private enum InvocationKind {
    case Awaitable
    case FireAndForget
    case UnsupportedSync
  }
}
