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

import golem.config.ConfigBuilder
import golem.runtime.Snapshotting
import golem.runtime.AgentImplementationType
import golem.schema.IntoSchema

import scala.reflect.macros.blackbox

// format: off
object AgentImplementationMacro {
  def implementationType[Trait](build: => Trait): AgentImplementationType[Trait, Unit] =
    macro AgentImplementationMacroImpl.implementationTypeImpl[Trait]

  def implementationTypeWithCtor[Trait, Ctor](
    build: Ctor => Trait
  ): AgentImplementationType[Trait, Ctor] =
    macro AgentImplementationMacroImpl.implementationTypeWithCtorImpl[Trait, Ctor]

  def implementationTypeFromClass[Trait, Impl <: Trait]: AgentImplementationType[Trait, Any] =
    macro AgentImplementationMacroImpl.implementationTypeFromClassImpl[Trait, Impl]
}

object AgentImplementationMacroImpl {
  private val schemaHint: String =
    "\nHint: IntoSchema/FromSchema are derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  def implementationTypeImpl[Trait: c.WeakTypeTag](c: blackbox.Context)(
    build: c.Expr[Trait]
  ): c.Expr[AgentImplementationType[Trait, Unit]] = {
    import c.universe._

    val traitType   = weakTypeOf[Trait]
    val traitSymbol = traitType.typeSymbol

    if (!traitSymbol.isClass || !traitSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"@agentImplementation target must be a trait, found: ${traitSymbol.fullName}")
    }

    val metadataExpr      = q"_root_.golem.runtime.macros.AgentDefinitionMacro.generate[$traitType]"
    val methodsExpr       = buildImplementationMethodsExpr(c)(traitType, metadataExpr)
    val configBuilderExpr = detectConfigBuilder(c)(traitType)

    c.Expr[AgentImplementationType[Trait, Unit]](q"""
      val metadata = $metadataExpr
      _root_.golem.runtime.AgentImplementationType[$traitType, _root_.scala.Unit](
        metadata = metadata,
        ctorCodec = _root_.golem.runtime.InputRecordCodec.unit,
        buildInstance = (_: _root_.scala.Unit, _: _root_.golem.Principal) => $build,
        methods = $methodsExpr,
        configBuilder = $configBuilderExpr
      )
    """)
  }

  def implementationTypeWithCtorImpl[Trait: c.WeakTypeTag, Ctor: c.WeakTypeTag](c: blackbox.Context)(
    build: c.Expr[Any]
  ): c.Expr[AgentImplementationType[Trait, Ctor]] = {
    import c.universe._

    val traitType   = weakTypeOf[Trait]
    val traitSymbol = traitType.typeSymbol

    if (!traitSymbol.isClass || !traitSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"@agentImplementation target must be a trait, found: ${traitSymbol.fullName}")
    }

    val idParams = agentInputParams(c)(traitType)
    val ctorType = weakTypeOf[Ctor]
    val gotCtor  = ctorType.dealias

    val ctorCodecExpr: Tree = idParams match {
      case Nil =>
        if (!(gotCtor =:= typeOf[Unit])) {
          c.abort(
            c.enclosingPosition,
            s"Constructor function input must be Unit for the empty Id class on ${traitSymbol.fullName} (found: $gotCtor)"
          )
        }
        q"_root_.golem.runtime.InputRecordCodec.unit"
      case (name, expected) :: Nil =>
        if (!(gotCtor =:= expected)) {
          c.abort(
            c.enclosingPosition,
            s"Constructor function input must match the Id class parameter ($expected) on ${traitSymbol.fullName} (found: $gotCtor)"
          )
        }
        val into = summonInto(c)(ctorType, s"constructor of ${traitSymbol.fullName}")
        val from = summonFrom(c)(ctorType, s"constructor of ${traitSymbol.fullName}")
        q"_root_.golem.runtime.InputRecordCodec.single[$ctorType]($name)($into, $from)"
      case _ =>
        c.abort(
          c.enclosingPosition,
          s"implementationType[Trait, Ctor] does not support multi-parameter constructors on " +
            s"${traitSymbol.fullName}. Use `implementationTypeFromClass` (or a single-field Id class) instead."
        )
    }

    val metadataExpr      = q"_root_.golem.runtime.macros.AgentDefinitionMacro.generate[$traitType]"
    val methodsExpr       = buildImplementationMethodsExpr(c)(traitType, metadataExpr)
    val configBuilderExpr = detectConfigBuilder(c)(traitType)

    c.Expr[AgentImplementationType[Trait, Ctor]](
      q"""
      val metadata = $metadataExpr
      _root_.golem.runtime.AgentImplementationType[$traitType, $ctorType](
        metadata = metadata,
        ctorCodec = $ctorCodecExpr,
        buildInstance = { val f = ($build).asInstanceOf[$ctorType => $traitType]; (input: $ctorType, _: _root_.golem.Principal) => f(input) },
        methods = $methodsExpr,
        configBuilder = $configBuilderExpr
      )
      """
    )
  }

  private def buildImplementationMethodsExpr(c: blackbox.Context)(
    traitType: c.universe.Type,
    metadataExpr: c.Tree
  ): c.Tree = {
    import c.universe._

    val methods = traitType.decls.collect {
      case method: MethodSymbol if method.isAbstract && method.isMethod && method.name.toString != "new" =>
        method
    }.toList

    val methodExprs = methods.map { method =>
      val methodName         = method.name.toString
      val methodMetadataExpr =
        q"""
        $metadataExpr.methods.find(_.name == $methodName).getOrElse {
          throw new IllegalStateException("Method metadata missing for " + $methodName)
        }
      """

      val allParams = method.paramLists.flatten.collect {
        case param if param.isTerm => (param.name.toTermName, param.typeSignature)
      }

      val principalFullName = "golem.Principal"
      val nonPrincipalParams = allParams.filter { case (_, tpe) =>
        tpe.dealias.typeSymbol.fullName != principalFullName
      }

      val (isAsync, payloadType) = methodReturnInfo(c)(method)
      val accessMode             = paramAccessMode(nonPrincipalParams)
      val inputType              = inputTypeFor(c)(accessMode, nonPrincipalParams)

      buildImplementationMethod(c)(traitType, method, methodMetadataExpr, allParams, nonPrincipalParams, accessMode, inputType, payloadType, isAsync)
    }

    q"List(..$methodExprs)"
  }

  private def buildImplementationMethod(c: blackbox.Context)(
    traitType: c.universe.Type,
    method: c.universe.MethodSymbol,
    methodMetadataExpr: c.Tree,
    allParams: List[(c.universe.TermName, c.universe.Type)],
    nonPrincipalParams: List[(c.universe.TermName, c.universe.Type)],
    accessMode: ParamAccessMode,
    inputType: c.universe.Type,
    outputType: c.universe.Type,
    isAsync: Boolean
  ): c.Tree = {
    import c.universe._

    val methodName = method.name.toString

    val inputCodecExprV  = inputCodecExpr(c)(accessMode, s"method $methodName", nonPrincipalParams.map { case (n, t) => (n.toString, t) })
    val outputCodecExprV = outputCodecExpr(c)(outputType, s"method $methodName")

    val handlerExpr = buildHandler(c)(traitType, method, allParams, nonPrincipalParams, accessMode, inputType, isAsync)

    if (isAsync) {
      q"""
        _root_.golem.runtime.AsyncImplementationMethod[$traitType, $inputType, $outputType](
          metadata = $methodMetadataExpr,
          inputCodec = $inputCodecExprV,
          outputCodec = $outputCodecExprV,
          handler = $handlerExpr
        )
      """
    } else {
      q"""
        _root_.golem.runtime.SyncImplementationMethod[$traitType, $inputType, $outputType](
          metadata = $methodMetadataExpr,
          inputCodec = $inputCodecExprV,
          outputCodec = $outputCodecExprV,
          handler = $handlerExpr
        )
      """
    }
  }

  private def buildHandler(c: blackbox.Context)(
    traitType: c.universe.Type,
    method: c.universe.MethodSymbol,
    allParams: List[(c.universe.TermName, c.universe.Type)],
    nonPrincipalParams: List[(c.universe.TermName, c.universe.Type)],
    accessMode: ParamAccessMode,
    inputType: c.universe.Type,
    isAsync: Boolean
  ): c.Tree = {
    import c.universe._

    val instanceName   = TermName("instance")
    val inputName      = TermName("input")
    val principalName  = TermName("principal")
    val methodCallName = method.name

    val principalFullName = "golem.Principal"

    val callExpr = accessMode match {
      case ParamAccessMode.NoArgs =>
        if (allParams.exists(_._2.dealias.typeSymbol.fullName == principalFullName)) {
          val argExprs = allParams.map { case (_, paramType) =>
            if (paramType.dealias.typeSymbol.fullName == principalFullName) q"$principalName"
            else throw new IllegalStateException("NoArgs should only have Principal params here")
          }
          q"$instanceName.$methodCallName(..$argExprs)"
        } else {
          q"$instanceName.$methodCallName()"
        }
      case ParamAccessMode.SingleArg =>
        if (allParams.exists(_._2.dealias.typeSymbol.fullName == principalFullName)) {
          val argExprs = allParams.map { case (_, paramType) =>
            if (paramType.dealias.typeSymbol.fullName == principalFullName) q"$principalName"
            else q"$inputName"
          }
          q"$instanceName.$methodCallName(..$argExprs)"
        } else {
          q"$instanceName.$methodCallName($inputName)"
        }
      case ParamAccessMode.MultiArgs =>
        val expectedCount = nonPrincipalParams.length
        var nonPrincipalIdx = 0
        val argExprs = allParams.map { case (_, paramType) =>
          if (paramType.dealias.typeSymbol.fullName == principalFullName) {
            q"$principalName"
          } else {
            val idx = nonPrincipalIdx
            nonPrincipalIdx += 1
            q"$inputName($idx).asInstanceOf[$paramType]"
          }
        }
        q"""
          if ($inputName.length != $expectedCount)
            throw new IllegalArgumentException(
              "Parameter count mismatch when invoking method '" + ${method.name.toString} + "'. Expected " + $expectedCount + "."
            )
          $instanceName.$methodCallName(..$argExprs)
        """
    }

    if (isAsync) {
      q"($instanceName: $traitType, $inputName: $inputType, $principalName: _root_.golem.Principal) => $callExpr"
    } else {
      q"($instanceName: $traitType, $inputName: $inputType, $principalName: _root_.golem.Principal) => $callExpr"
    }
  }

  private def methodReturnInfo(c: blackbox.Context)(
    method: c.universe.MethodSymbol
  ): (Boolean, c.universe.Type) = {
    import c.universe._

    val returnType   = method.returnType
    val futureSymbol = typeOf[scala.concurrent.Future[_]].typeSymbol

    returnType match {
      case TypeRef(_, sym, args) if sym == futureSymbol && args.nonEmpty =>
        (true, args.head)
      case _ =>
        (false, returnType)
    }
  }

  private def paramAccessMode(params: List[(_, _)]): ParamAccessMode = params match {
    case Nil      => ParamAccessMode.NoArgs
    case _ :: Nil => ParamAccessMode.SingleArg
    case _        => ParamAccessMode.MultiArgs
  }

  private def inputTypeFor(c: blackbox.Context)(
    accessMode: ParamAccessMode,
    params: List[(_, c.universe.Type)]
  ): c.universe.Type = {
    import c.universe._
    accessMode match {
      case ParamAccessMode.NoArgs    => typeOf[Unit]
      case ParamAccessMode.SingleArg => params.head._2
      case ParamAccessMode.MultiArgs => typeOf[Vector[Any]]
    }
  }

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

  private def detectConfigBuilder(c: blackbox.Context)(traitType: c.universe.Type): c.Tree = {
    import c.universe._

    val agentConfigBases = traitType.baseClasses.filter(_.fullName == "golem.config.AgentConfig")

    if (agentConfigBases.isEmpty) q"_root_.scala.None"
    else {
      val configTypes = agentConfigBases.flatMap { sym =>
        traitType.baseType(sym) match {
          case TypeRef(_, _, List(arg)) => Some(arg)
          case _                        => None
        }
      }
      configTypes.headOption match {
        case Some(configType) =>
          val cbTpe           = appliedType(typeOf[ConfigBuilder[_]].typeConstructor, configType)
          val builderImplicit = c.inferImplicitValue(cbTpe)
          if (builderImplicit.isEmpty) {
            c.abort(
              c.enclosingPosition,
              s"No implicit ConfigBuilder available for config type $configType.\n" +
                "Hint: Add an implicit Schema[T] for your config type, which provides ConfigBuilder automatically."
            )
          }
          q"_root_.scala.Some($builderImplicit: _root_.golem.config.ConfigBuilder[_])"
        case None =>
          q"_root_.scala.None"
      }
    }
  }

  def implementationTypeFromClassImpl[Trait: c.WeakTypeTag, Impl: c.WeakTypeTag](c: blackbox.Context): c.Expr[AgentImplementationType[Trait, Any]] = {
    import c.universe._

    val traitType   = weakTypeOf[Trait]
    val traitSymbol = traitType.typeSymbol

    if (!traitSymbol.isClass || !traitSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"@agentImplementation target must be a trait, found: ${traitSymbol.fullName}")
    }

    val implType   = weakTypeOf[Impl]
    val implSymbol = implType.typeSymbol

    if (!implSymbol.isClass) {
      c.abort(c.enclosingPosition, s"Impl type must be a concrete class, found: ${implSymbol.fullName}")
    }
    val implClass = implSymbol.asClass
    if (implClass.isAbstract) {
      c.abort(c.enclosingPosition, s"Impl type must be a concrete class, found abstract: ${implSymbol.fullName}")
    }
    if (implClass.isTrait) {
      c.abort(c.enclosingPosition, s"Impl type must be a concrete class, found trait: ${implSymbol.fullName}")
    }
    if (implClass.isModuleClass || implSymbol.isModule) {
      c.abort(c.enclosingPosition, s"Impl type must be a concrete class, found object: ${implSymbol.fullName}")
    }

    val primaryCtors = implType.decls.collect {
      case m: MethodSymbol if m.isConstructor && m.isPrimaryConstructor => m
    }.toList
    if (primaryCtors.isEmpty) {
      c.abort(c.enclosingPosition, s"Impl type ${implSymbol.fullName} has no accessible primary constructor")
    }
    val primaryCtor = primaryCtors.head
    val termParamLists = primaryCtor.paramLists.filter(_.forall(_.isTerm))
    if (termParamLists.length != 1) {
      c.abort(
        c.enclosingPosition,
        s"Impl type ${implSymbol.fullName} must have exactly one term parameter list, found ${termParamLists.length}"
      )
    }

    val params: List[(TermName, Type)] = termParamLists.head.map { sym =>
      (sym.name.toTermName, sym.typeSignature)
    }

    val configFullName    = "golem.config.Config"
    val principalFullName = "golem.Principal"

    case class ParamInfo(name: TermName, tpe: Type, index: Int, isConfig: Boolean, isPrincipal: Boolean, configInnerType: Option[Type])

    val paramInfos: List[ParamInfo] = params.zipWithIndex.map { case ((name, tpe), idx) =>
      val dealiased = tpe.dealias
      if (dealiased.typeSymbol.fullName == configFullName && dealiased.typeArgs.nonEmpty) {
        ParamInfo(name, tpe, idx, isConfig = true, isPrincipal = false, configInnerType = Some(dealiased.typeArgs.head))
      } else if (dealiased.typeSymbol.fullName == principalFullName) {
        ParamInfo(name, tpe, idx, isConfig = false, isPrincipal = true, configInnerType = None)
      } else {
        ParamInfo(name, tpe, idx, isConfig = false, isPrincipal = false, configInnerType = None)
      }
    }

    val configParams    = paramInfos.filter(_.isConfig)
    val principalParams = paramInfos.filter(_.isPrincipal)
    val identityParams  = paramInfos.filter(p => !p.isConfig && !p.isPrincipal)

    if (configParams.length > 1) {
      c.abort(
        c.enclosingPosition,
        s"Impl type ${implSymbol.fullName} has ${configParams.length} Config[_] parameters, at most one is allowed"
      )
    }

    if (principalParams.length > 1) {
      c.abort(
        c.enclosingPosition,
        s"Impl type ${implSymbol.fullName} has ${principalParams.length} Principal parameters, at most one is allowed"
      )
    }

    val principalParam = principalParams.headOption

    // The user-supplied Id-class params (Principal filtered out) are the source
    // of truth for the constructor input record; validate the impl's identity
    // params against them.
    val idParams = agentInputParams(c)(traitType)

    idParams match {
      case Nil =>
        if (identityParams.nonEmpty) {
          c.abort(
            c.enclosingPosition,
            s"Trait ${traitSymbol.fullName} has an empty Id class (Unit constructor), " +
              s"but Impl ${implSymbol.fullName} has ${identityParams.length} non-Config constructor parameter(s): " +
              s"${identityParams.map(_.name.toString).mkString(", ")}"
          )
        }
      case (_, expected) :: Nil =>
        if (identityParams.length == 1) {
          if (!(identityParams.head.tpe =:= expected)) {
            c.abort(
              c.enclosingPosition,
              s"Constructor parameter '${identityParams.head.name}' has type ${identityParams.head.tpe}, " +
                s"but Id class expects $expected"
            )
          }
        } else if (identityParams.length > 1) {
          c.abort(
            c.enclosingPosition,
            s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
              s"Id class declares a single constructor parameter"
          )
        }
      // identityParams.isEmpty is valid (config-only constructor on a non-Unit Id class)
      case multi =>
        if (identityParams.nonEmpty) {
          if (multi.length != identityParams.length) {
            c.abort(
              c.enclosingPosition,
              s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
                s"Id class declares ${multi.length} constructor parameter(s)"
            )
          }
          identityParams.zip(multi).foreach { case (param, (_, expected)) =>
            if (!(param.tpe =:= expected)) {
              c.abort(
                c.enclosingPosition,
                s"Constructor parameter '${param.name}' has type ${param.tpe}, " +
                  s"expected $expected (from Id class parameters)"
              )
            }
          }
        }
      // identityParams.isEmpty is valid (config-only constructor on a non-Unit Id class)
    }

    // Determine the Ctor type + wire access mode from the Id-class params (the
    // source of truth for the constructor input record). Multi-param ctors are
    // represented positionally as `Vector[Any]`, matching method inputs.
    val ctorAccess: ParamAccessMode = idParams match {
      case Nil      => ParamAccessMode.NoArgs
      case _ :: Nil => ParamAccessMode.SingleArg
      case _        => ParamAccessMode.MultiArgs
    }
    val ctorType: Type = ctorAccess match {
      case ParamAccessMode.NoArgs    => typeOf[Unit]
      case ParamAccessMode.SingleArg => idParams.head._2
      case ParamAccessMode.MultiArgs => typeOf[Vector[Any]]
    }

    val ctorCodecExpr = inputCodecExpr(c)(ctorAccess, s"constructor of ${traitSymbol.fullName}", idParams)

    val metadataExpr = q"_root_.golem.runtime.macros.AgentDefinitionMacro.generate[$traitType]"
    val methodsExpr  = buildImplementationMethodsExpr(c)(traitType, metadataExpr)

    // Resolve configBuilder
    val configParam = configParams.headOption

    val configBuilderExpr: Tree = configParam match {
      case Some(cp) =>
        val configInner = cp.configInnerType.get
        val agentConfigBases = traitType.baseClasses.filter(_.fullName == "golem.config.AgentConfig")
        if (agentConfigBases.isEmpty) {
          c.abort(
            c.enclosingPosition,
            s"Impl ${implSymbol.fullName} has a Config[$configInner] parameter, " +
              s"but trait ${traitSymbol.fullName} does not extend AgentConfig"
          )
        }

        val configTypes = agentConfigBases.flatMap { sym =>
          traitType.baseType(sym) match {
            case TypeRef(_, _, List(arg)) => Some(arg)
            case _                       => None
          }
        }

        configTypes.headOption match {
          case Some(agentConfigType) =>
            if (!(configInner =:= agentConfigType)) {
              c.abort(
                c.enclosingPosition,
                s"Config parameter type Config[$configInner] does not match " +
                  s"AgentConfig[$agentConfigType] on trait ${traitSymbol.fullName}"
              )
            }
            val cbTpe = appliedType(typeOf[ConfigBuilder[_]].typeConstructor, configInner)
            val builderImplicit = c.inferImplicitValue(cbTpe)
            if (builderImplicit.isEmpty) {
              c.abort(
                c.enclosingPosition,
                s"No implicit ConfigBuilder available for config type $configInner.\n" +
                  "Hint: Add an implicit Schema[T] for your config type, which provides ConfigBuilder automatically."
              )
            }
            q"_root_.scala.Some($builderImplicit: _root_.golem.config.ConfigBuilder[_])"
          case None =>
            c.abort(
              c.enclosingPosition,
              s"Trait ${traitSymbol.fullName} extends AgentConfig but type argument could not be extracted"
            )
        }

      case None =>
        detectConfigBuilder(c)(traitType)
    }

    // Build the instance construction lambda: (Ctor, Principal) => Trait
    val inputTermName    = TermName("input")
    val principalArgName = TermName("principalArg")

    def identityArgTree(pi: ParamInfo): Tree =
      identityParams match {
        case Nil      => c.abort(c.enclosingPosition, "Unexpected: no identity params but trying to construct args")
        case _ :: Nil => q"$inputTermName"
        case ps       =>
          val idx = ps.indexWhere(_.index == pi.index)
          q"$inputTermName($idx).asInstanceOf[${pi.tpe}]"
      }

    val buildInstanceExpr: Tree = configParam match {
      case None =>
        val argTerms: List[Tree] = paramInfos.map { pi =>
          if (pi.isPrincipal) q"$principalArgName"
          else identityArgTree(pi)
        }
        q"($inputTermName: $ctorType, $principalArgName: _root_.golem.Principal) => new $implType(..$argTerms)"

      case Some(cp) =>
        val configInner = cp.configInnerType.get
        val cbTpe = appliedType(typeOf[ConfigBuilder[_]].typeConstructor, configInner)
        val builderImplicit = c.inferImplicitValue(cbTpe)
        val argTerms: List[Tree] = paramInfos.map { pi =>
          if (pi.isConfig) {
            q"_root_.golem.config.ConfigLoader.createLazyConfig[$configInner]($builderImplicit)"
          } else if (pi.isPrincipal) {
            q"$principalArgName"
          } else identityArgTree(pi)
        }
        q"($inputTermName: $ctorType, $principalArgName: _root_.golem.Principal) => new $implType(..$argTerms)"
    }

    c.Expr[AgentImplementationType[Trait, Any]](
      q"""
      {
        val metadata = $metadataExpr
        _root_.golem.runtime.AgentImplementationType[$traitType, $ctorType](
          metadata = metadata,
          ctorCodec = $ctorCodecExpr,
          buildInstance = $buildInstanceExpr,
          methods = $methodsExpr,
          configBuilder = $configBuilderExpr,
          configInjectedViaConstructor = ${if (configParam.isDefined) q"true" else q"false"},
          principalInjectedViaConstructor = ${if (principalParam.isDefined) q"true" else q"false"},
          snapshotHandlers = ${buildSnapshotHandlersExpr(c)(traitType, implType)}
        ).asInstanceOf[_root_.golem.runtime.AgentImplementationType[$traitType, Any]]
      }
      """
    )
  }

  private def buildSnapshotHandlersExpr(c: blackbox.Context)(
    traitType: c.universe.Type,
    implType: c.universe.Type
  ): c.Tree = {
    import c.universe._

    val customHooks      = detectCustomSnapshotHooks(c)(implType)
    val snapshottedState = detectSnapshottedStateType(c)(implType)
    val snapshottingEnabled = extractSnapshottingEnabled(c)(traitType)

    customHooks match {
      case Some((saveSym, loadSym)) =>
        // Custom saveSnapshot/loadSnapshot hooks
        q"""
        {
          val rawSave: ($traitType => _root_.scala.concurrent.Future[Array[Byte]]) =
            (instance: $traitType) => instance.asInstanceOf[$implType].${saveSym.name.toTermName}()
          val rawLoad: (($traitType, Array[Byte]) => _root_.scala.concurrent.Future[Unit]) =
            (instance: $traitType, bytes: Array[Byte]) => instance.asInstanceOf[$implType].${loadSym.name.toTermName}(bytes)
          _root_.scala.Some(
            _root_.golem.runtime.SnapshotHandlers[$traitType](
              save = _root_.golem.runtime.SnapshotHandlers.wrapSave[$traitType](rawSave),
              load = _root_.golem.runtime.SnapshotHandlers.wrapLoad[$traitType](rawLoad)
            )
          )
        }
        """
      case None =>
        snapshottedState match {
          case Some(stateTpe) =>
            // Snapshotted[S] mixin — call stateSchema on the instance
            q"""
            {
              _root_.scala.Some(
                _root_.golem.runtime.SnapshotHandlers[$traitType](
                  save = (instance: $traitType) => {
                    val snap = instance.asInstanceOf[_root_.golem.Snapshotted[$stateTpe]]
                    val codec = snap.stateSchema.derive(_root_.zio.blocks.schema.json.JsonCodecDeriver)
                    _root_.scala.concurrent.Future.successful(
                      _root_.golem.runtime.SnapshotPayload(
                        bytes = codec.encode(snap.state),
                        mimeType = "application/json"
                      )
                    )
                  },
                  load = (instance: $traitType, bytes: Array[Byte]) => {
                    val snap = instance.asInstanceOf[_root_.golem.Snapshotted[$stateTpe]]
                    val codec = snap.stateSchema.derive(_root_.zio.blocks.schema.json.JsonCodecDeriver)
                    codec.decode(bytes) match {
                      case Right(restored) =>
                        snap.state = restored
                        _root_.scala.concurrent.Future.successful(instance)
                      case Left(err) =>
                        _root_.scala.concurrent.Future.failed(
                          new IllegalArgumentException(
                            "Failed to decode JSON snapshot for " + ${Literal(Constant(implType.typeSymbol.fullName))} + ": " + err
                          )
                        )
                    }
                  }
                )
              )
            }
            """
          case None =>
            if (snapshottingEnabled) {
              c.abort(
                c.enclosingPosition,
                s"Snapshotting is enabled for ${traitType.typeSymbol.fullName}, but ${implType.typeSymbol.fullName} " +
                s"provides no snapshot support. Either:\n" +
                s"  (1) Mix in Snapshotted[S] and implement `stateSchema` with your Schema[S] instance\n" +
                s"  (2) Implement `def saveSnapshot(): Future[Array[Byte]]` and `def loadSnapshot(bytes: Array[Byte]): Future[Unit]`"
              )
            }
            q"_root_.scala.None"
        }
    }
  }

  private def extractSnapshottingEnabled(c: blackbox.Context)(
    traitType: c.universe.Type
  ): Boolean = {
    import c.universe._

    val agentDefinitionFQN = "golem.runtime.annotations.agentDefinition"
    def isAgentDefinitionAnn(ann: Annotation): Boolean =
      ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN
    val traitSymbol = traitType.typeSymbol

    val snapStr = traitSymbol.annotations.collectFirst {
      case ann if isAgentDefinitionAnn(ann) =>
        val args = ann.tree.children.tail
        args.collectFirst {
          case NamedArg(Ident(TermName("snapshotting")), Literal(Constant(v: String))) => v
        }.orElse {
          args.lift(7).collect {
            case Literal(Constant(v: String)) => v
            case NamedArg(_, Literal(Constant(v: String))) => v
          }
        }
    }.flatten.getOrElse("disabled")

    Snapshotting.parse(snapStr) match {
      case Right(Snapshotting.Enabled(_)) => true
      case _                             => false
    }
  }

  private def detectCustomSnapshotHooks(c: blackbox.Context)(
    implType: c.universe.Type
  ): Option[(c.universe.MethodSymbol, c.universe.MethodSymbol)] = {
    import c.universe._

    val futureSymbol = typeOf[scala.concurrent.Future[_]].typeSymbol

    val saveMatches = implType.decls.collect {
      case m: MethodSymbol
        if m.name.decodedName.toString == "saveSnapshot" &&
           !m.isPrivate && !m.isProtected &&
           m.paramLists.flatten.filter(_.isTerm).isEmpty &&
           m.returnType.typeSymbol == futureSymbol =>
        m
    }.toList

    val loadMatches = implType.decls.collect {
      case m: MethodSymbol
        if m.name.decodedName.toString == "loadSnapshot" &&
           !m.isPrivate && !m.isProtected && {
             val termParams = m.paramLists.flatten.filter(_.isTerm)
             termParams.length == 1 && termParams.head.typeSignature =:= typeOf[Array[Byte]]
           } &&
           m.returnType.typeSymbol == futureSymbol =>
        m
    }.toList

    if (saveMatches.nonEmpty != loadMatches.nonEmpty)
      c.abort(
        c.enclosingPosition,
        s"${implType.typeSymbol.fullName} must declare both saveSnapshot and loadSnapshot, or neither"
      )

    for {
      save <- saveMatches.headOption
      load <- loadMatches.headOption
    } yield (save, load)
  }

  private def detectSnapshottedStateType(c: blackbox.Context)(
    implType: c.universe.Type
  ): Option[c.universe.Type] = {
    import c.universe._

    val snapSymName = "golem.Snapshotted"
    val snapBaseOpt = implType.baseClasses.find(_.fullName == snapSymName)

    snapBaseOpt.flatMap { snapSym =>
      implType.baseType(snapSym) match {
        case TypeRef(_, _, List(stateTpe)) => Some(stateTpe)
        case _                            => None
      }
    }
  }

  private sealed trait ParamAccessMode

  private object ParamAccessMode {
    case object NoArgs extends ParamAccessMode

    case object SingleArg extends ParamAccessMode

    case object MultiArgs extends ParamAccessMode
  }
}
