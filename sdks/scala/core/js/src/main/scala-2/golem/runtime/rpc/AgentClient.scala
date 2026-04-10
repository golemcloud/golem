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

import golem.runtime.AgentType
import scala.language.experimental.macros

// format: off
object AgentClient {
  /**
   * Resolves an agent id + RPC invoker for a trait + constructor args.
   *
   * In Scala 2 (Scala.js), prefer: 1)
   * `val agentType = AgentClient.agentType[MyAgent]` 2)
   * `val resolved = AgentClient.resolve(agentType, ctorArgs)` 3)
   * `val client = AgentClient.bind[MyAgent](resolved)`
   *
   * This avoids relying on Java reflection proxies (which Scala.js doesn't
   * support).
   */
  def resolve[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor
  ): Either[String, AgentClientRuntime.ResolvedAgent[Trait]] =
    AgentClientRuntime.resolve(agentType, constructorArgs)

  def bind[Trait](
    resolved: AgentClientRuntime.ResolvedAgent[Trait]
  ): Trait = macro AgentClientBindMacro.bindImpl[Trait]

  def agentType[Trait]: AgentType[Trait, _] = macro golem.runtime.macros.AgentClientMacroImpl.agentTypeImpl[Trait]

  /**
   * Typed agent-type accessor (no user-land casts).
   *
   * Validates at compile-time that `Constructor` matches
   * the `BaseAgent` constructor type on the agent trait.
   */
  def agentTypeWithCtor[Trait, Constructor]: AgentType[Trait, Constructor] =
    macro AgentTypeWithCtorMacro.agentTypeWithCtorImpl[Trait, Constructor]
}

private[rpc] object AgentTypeWithCtorMacro {
  def agentTypeWithCtorImpl[Trait: c.WeakTypeTag, Constructor: c.WeakTypeTag](
    c: scala.reflect.macros.blackbox.Context
  ): c.Expr[AgentType[Trait, Constructor]] = {
    import c.universe._

    val traitType   = weakTypeOf[Trait].dealias
    val traitSymbol = traitType.typeSymbol

    if (!traitSymbol.isClass || !traitSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"Agent client target must be a trait, found: ${traitSymbol.fullName}")
    }

    val expectedCtor: Type = typeOf[Unit].dealias

    val gotCtor = weakTypeOf[Constructor].dealias

    if (!(gotCtor =:= expectedCtor)) {
      c.abort(
        c.enclosingPosition,
        s"AgentClient.agentTypeWithCtor requires: BaseAgent[$expectedCtor] (found: $gotCtor)"
      )
    }

    c.Expr[AgentType[Trait, Constructor]](
      q"_root_.golem.runtime.rpc.AgentClient.agentType[$traitType].asInstanceOf[_root_.golem.runtime.AgentType[$traitType, $gotCtor]]"
    )
  }
}

private[rpc] object AgentClientBindMacro {
  def bindImpl[Trait: c.WeakTypeTag](c: scala.reflect.macros.blackbox.Context)(
    resolved: c.Expr[AgentClientRuntime.ResolvedAgent[Trait]]
  ): c.Expr[Trait] = {
    import c.universe._

    val traitTpe = weakTypeOf[Trait]
    val traitSym = traitTpe.typeSymbol

    if (!traitSym.isClass || !traitSym.asClass.isTrait)
      c.abort(c.enclosingPosition, s"Agent client target must be a trait, found: ${traitSym.fullName}")

    val futureSym        = typeOf[scala.concurrent.Future[_]].typeSymbol
    val principalFullName = "golem.Principal"

    def isPrincipalType(tpe: Type): Boolean =
      tpe.dealias.typeSymbol.fullName == principalFullName

    def isPromiseReturn(tpe: Type): Boolean =
      tpe.typeSymbol.fullName == "scala.scalajs.js.Promise"

    def isFutureReturn(tpe: Type): Boolean =
      tpe.typeSymbol == futureSym

    def unwrapAsync(tpe: Type): Type =
      tpe match {
        case TypeRef(_, sym, List(arg)) if sym == futureSym                           => arg
        case TypeRef(_, sym, List(arg)) if sym.fullName == "scala.scalajs.js.Promise" => arg
        case other                                                                    => other
      }

    val methods: List[MethodSymbol] =
      traitTpe.decls.collect {
        case m: MethodSymbol if m.isAbstract && m.isMethod && m.name.toString != "new" => m
      }.toList

    val resolvedValName = TermName(c.freshName("resolved"))
    val resolvedValDef  = q"val $resolvedValName = $resolved"
    val resolvedRef     = Ident(resolvedValName)

    def methodLookup(methodName: String, inTpe: Type, outTpe: Type): Tree =
      q"""
        $resolvedRef.agentType.methods
          .collectFirst {
            case p if p.metadata.name == $methodName =>
              p.asInstanceOf[_root_.golem.runtime.AgentMethod[$traitTpe, $inTpe, $outTpe]]
          }
          .getOrElse(throw new _root_.java.lang.IllegalStateException("Method definition for " + $methodName + " not found"))
      """

    def inputExpr(paramss: List[List[ValDef]]): Tree = {
      val params = paramss.flatten.filter(p => !isPrincipalType(p.tpt.tpe))
      params match {
        case Nil        => q"()"
        case one :: Nil => q"${Ident(one.name)}"
        case many       =>
          q"_root_.scala.collection.immutable.Vector(..${many.map(p => Ident(p.name))})"
      }
    }

    def mkParamValDef(p: Symbol): ValDef = {
      val name = p.name.toTermName
      val tpt  = TypeTree(p.typeSignature)
      ValDef(Modifiers(Flag.PARAM), name, tpt, EmptyTree)
    }

    def mkMethodDef(m: MethodSymbol, rhs: Tree): DefDef = {
      val name    = m.name.toTermName
      val tparams = Nil
      val paramss = m.paramLists.map(_.map(mkParamValDef))
      val retTpt  = TypeTree(m.returnType)
      DefDef(Modifiers(Flag.OVERRIDE), name, tparams, paramss, retTpt, rhs)
    }

    val methodDefs: List[DefDef] = methods.map { m =>
      val methodNameStr               = m.name.toString
      val paramss: List[List[ValDef]] = m.paramLists.map(_.map(mkParamValDef))
      val returnTpe                   = m.returnType

      val nonPrincipalParams = m.paramLists.flatten.filter(p => !isPrincipalType(p.typeSignature))

      val inType: Type =
        nonPrincipalParams match {
          case Nil        => typeOf[Unit]
          case one :: Nil => one.typeSignature
          case _          => typeOf[Vector[Any]]
        }

      if (returnTpe =:= typeOf[Unit]) {
        val methodLookup0 = methodLookup(methodNameStr, inType, typeOf[Unit])
        val inValue       = inputExpr(paramss)
        val rhs           =
          q"""
            val method = $methodLookup0
            $resolvedRef
              .trigger(method, $inValue.asInstanceOf[$inType])
              .failed
              .foreach(err => _root_.scala.scalajs.js.Dynamic.global.console.error("RPC trigger " + $methodNameStr + " failed", err.asInstanceOf[_root_.scala.scalajs.js.Any]))
            ()
          """
        mkMethodDef(m, rhs)
      } else if (isFutureReturn(returnTpe) || isPromiseReturn(returnTpe)) {
        val outType       = unwrapAsync(returnTpe)
        val methodLookup0 = methodLookup(methodNameStr, inType, outType)
        val inValue       = inputExpr(paramss)
        val rhs           =
          q"""
            val method = $methodLookup0
            $resolvedRef.call(method, $inValue.asInstanceOf[$inType])
          """
        mkMethodDef(m, rhs)
      } else {
        val rhs =
          q"""throw new _root_.java.lang.IllegalStateException(
                "Agent client method " + $methodNameStr + " must return scala.concurrent.Future[...] or Unit when invoked via RPC."
              )"""
        mkMethodDef(m, rhs)
      }
    }

    val anon =
      q"""
        new $traitTpe {
          ..$methodDefs
        }
      """

    c.Expr[Trait](q"{ $resolvedValDef; $anon }")
  }
}
