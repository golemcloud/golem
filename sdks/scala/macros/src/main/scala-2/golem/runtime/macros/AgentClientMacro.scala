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

import golem.runtime.AgentType
import golem.schema.IntoSchema
import scala.reflect.macros.blackbox

object AgentClientMacro {
  def agentType[Trait]: AgentType[Trait, _] = macro AgentClientMacroImpl.agentTypeImpl[Trait]
}

object AgentClientMacroImpl {
  private val schemaHint: String =
    "\nHint: IntoSchema/FromSchema are derived from zio.blocks.schema.Schema.\n" +
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

    val idParams   = agentInputParams(c)(traitType)
    val accessMode = paramAccessMode(idParams)
    val inputType  = inputTypeFor(c)(accessMode, idParams)

    val codecExpr = inputCodecExpr(c)(accessMode, "constructor", idParams)
    val typeExpr  = q"_root_.golem.runtime.ConstructorType[$inputType]($codecExpr)"
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

  /**
   * The user-supplied `class Id(...)` parameters (name + type), Principal
   * params filtered out. These define the constructor input record's shape.
   */
  private def agentInputParams(c: blackbox.Context)(
    traitType: c.universe.Type
  ): List[(String, c.universe.Type)] = {
    import c.universe._
    val idAnnotationType  = typeOf[golem.runtime.annotations.id]
    val principalFullName = "golem.Principal"

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
    primaryCtor.paramLists.flatten
      .filter(_.isTerm)
      .map(p => (p.name.toString, p.typeSignature))
      .filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != principalFullName }
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

    val params                       = extractParameters(c)(method)
    val accessMode                   = paramAccessMode(params)
    val inputType                    = inputTypeFor(c)(accessMode, params)
    val (invocationKind, outputType) = methodInvocationInfo(c)(method)

    val invocationExpr = invocationKind match {
      case InvocationKind.Awaitable     => q"_root_.golem.runtime.MethodInvocation.Awaitable"
      case InvocationKind.FireAndForget => q"_root_.golem.runtime.MethodInvocation.FireAndForget"
    }

    val inputCodecExprV =
      inputCodecExpr(c)(accessMode, s"method $methodName", params.map { case (n, t) => (n.toString, t) })
    val outputCodecExprV = outputCodecExpr(c)(outputType, s"method $methodName")

    q"""
      {
        val inputCodec  = $inputCodecExprV
        val outputCodec = $outputCodecExprV
        _root_.golem.runtime.AgentMethod[$traitType, $inputType, $outputType](
          metadata = _root_.golem.runtime.MethodMetadata(
            name = $methodName,
            description = _root_.scala.None,
            prompt = _root_.scala.None,
            mode = _root_.scala.None,
            input = inputCodec.inputMetadata,
            output = outputCodec.metadata
          ),
          functionName = $functionName,
          inputCodec = inputCodec,
          outputCodec = outputCodec,
          invocation = $invocationExpr
        )
      }
    """
  }

  private def extractParameters(c: blackbox.Context)(
    method: c.universe.MethodSymbol
  ): List[(c.universe.TermName, c.universe.Type)] = {
    import c.universe._
    val principalFullName = "golem.Principal"
    method.paramLists.flatten.collect {
      case param if param.isTerm => (param.name.toTermName, param.typeSignature)
    }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != principalFullName }
  }

  private def paramAccessMode(params: List[(_, _)]): ParamAccessMode = params match {
    case Nil      => ParamAccessMode.NoArgs
    case _ :: Nil => ParamAccessMode.SingleArg
    case _        => ParamAccessMode.MultiArgs
  }

  private def inputTypeFor(
    c: blackbox.Context
  )(accessMode: ParamAccessMode, params: List[(_, c.universe.Type)]): c.universe.Type = {
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

  private def summonInto(c: blackbox.Context)(tpe: c.universe.Type, position: String): c.Tree = {
    import c.universe._
    val intoType     = appliedType(typeOf[IntoSchema[_]].typeConstructor, tpe)
    val intoInstance = c.inferImplicitValue(intoType)
    if (intoInstance.isEmpty) {
      c.abort(c.enclosingPosition, s"Unable to summon IntoSchema for $position with type $tpe.$schemaHint")
    }
    intoInstance
  }

  private def summonFrom(c: blackbox.Context)(tpe: c.universe.Type, position: String): c.Tree = {
    import c.universe._
    val fromType     = appliedType(typeOf[golem.schema.FromSchema[_]].typeConstructor, tpe)
    val fromInstance = c.inferImplicitValue(fromType)
    if (fromInstance.isEmpty) {
      c.abort(c.enclosingPosition, s"Unable to summon FromSchema for $position with type $tpe.$schemaHint")
    }
    fromInstance
  }

  /**
   * Build the `InputRecordCodec[In]` for a constructor/method input from its
   * user-supplied parameters: `unit` (no args), `single` (one arg), or
   * `fromParams` (multiple args, encoded positionally as `Vector[Any]`).
   */
  private def inputCodecExpr(c: blackbox.Context)(
    accessMode: ParamAccessMode,
    context: String,
    params: List[(String, c.universe.Type)]
  ): c.Tree = {
    import c.universe._
    accessMode match {
      case ParamAccessMode.NoArgs =>
        q"_root_.golem.runtime.InputRecordCodec.unit"
      case ParamAccessMode.SingleArg =>
        val (name, tpe) = params.head
        val into        = summonInto(c)(tpe, s"input of $context")
        val from        = summonFrom(c)(tpe, s"input of $context")
        q"_root_.golem.runtime.InputRecordCodec.single[$tpe]($name)($into, $from)"
      case ParamAccessMode.MultiArgs =>
        val paramCodecs = paramCodecsExpr(c)(context, params)
        q"_root_.golem.runtime.InputRecordCodec.fromParams($paramCodecs)"
    }
  }

  private def paramCodecsExpr(c: blackbox.Context)(
    context: String,
    params: List[(String, c.universe.Type)]
  ): c.Tree = {
    import c.universe._
    val entries = params.map { case (name, tpe) =>
      val into = summonInto(c)(tpe, s"parameter '$name' of $context")
      val from = summonFrom(c)(tpe, s"parameter '$name' of $context")
      q"""
        _root_.golem.runtime.ParamCodec(
          $name,
          $into.asInstanceOf[_root_.golem.schema.IntoSchema[Any]],
          $from.asInstanceOf[_root_.golem.schema.FromSchema[Any]]
        )
      """
    }
    q"_root_.scala.List(..$entries)"
  }

  /**
   * Build the `OutputCodec[Out]` for a method's return type: `unit` for `Unit`
   * (the host returns `none`), otherwise `single` carrying the value codec.
   */
  private def outputCodecExpr(c: blackbox.Context)(tpe: c.universe.Type, context: String): c.Tree = {
    import c.universe._
    if (tpe =:= typeOf[Unit]) q"_root_.golem.runtime.OutputCodec.unit[$tpe]"
    else {
      val into = summonInto(c)(tpe, s"output of $context")
      val from = summonFrom(c)(tpe, s"output of $context")
      q"_root_.golem.runtime.OutputCodec.single[$tpe]($into, $from)"
    }
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
