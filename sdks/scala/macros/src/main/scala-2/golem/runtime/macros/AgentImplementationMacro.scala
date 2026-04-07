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

import golem.config.ConfigBuilder
import golem.data.GolemSchema
import golem.runtime.Snapshotting
import golem.runtime.AgentImplementationType

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
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
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

    val metadataExpr = q"_root_.golem.runtime.macros.AgentDefinitionMacro.generate[$traitType]"
    val methodsExpr  = buildImplementationMethodsExpr(c)(traitType, metadataExpr)

    val ctorSchemaExpr =
      c.inferImplicitValue(appliedType(typeOf[GolemSchema[_]].typeConstructor, typeOf[Unit]))

    c.Expr[AgentImplementationType[Trait, Unit]](q"""
      val metadata = $metadataExpr
      _root_.golem.runtime.AgentImplementationType[$traitType, _root_.scala.Unit](
        metadata = metadata,
        idSchema = $ctorSchemaExpr,
        buildInstance = (_: _root_.scala.Unit, _: _root_.golem.Principal) => $build,
        methods = $methodsExpr
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

    val ctorType: Type = {
      val idAnnotationType = typeOf[golem.runtime.annotations.id]

      val annotatedClass = traitType.members.collectFirst {
        case sym if sym.isClass && !sym.isMethod &&
          sym.annotations.exists(ann => ann.tree.tpe != null && ann.tree.tpe =:= idAnnotationType) =>
          sym
      }

      val constructorClass = annotatedClass.orElse {
        val byName = traitType.member(TypeName("Id"))
        if (byName == NoSymbol) None else Some(byName)
      }.getOrElse {
        c.abort(c.enclosingPosition,
          s"Agent trait ${traitSymbol.fullName} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters.")
      }
      val primaryCtor = constructorClass.asClass.primaryConstructor.asMethod
      val params = primaryCtor.paramLists.flatten.filter(_.isTerm).map(_.typeSignature)
      params match {
        case Nil      => typeOf[Unit]
        case p :: Nil => p
        case ps       =>
          val tupleClass = rootMirror.staticClass(s"scala.Tuple${ps.length}")
          appliedType(tupleClass.toType, ps)
      }
    }

    val gotCtor = weakTypeOf[Ctor].dealias
    if (!(gotCtor =:= ctorType)) {
      c.abort(
        c.enclosingPosition,
        s"Constructor function must have input type matching Id class parameters ($ctorType) on ${traitSymbol.fullName} (found: $gotCtor)"
      )
    }

    val metadataExpr = q"_root_.golem.runtime.macros.AgentDefinitionMacro.generate[$traitType]"
    val methodsExpr  = buildImplementationMethodsExpr(c)(traitType, metadataExpr)

    val ctorSchemaTpe  = appliedType(typeOf[GolemSchema[_]].typeConstructor, ctorType)
    val ctorSchemaExpr = c.inferImplicitValue(ctorSchemaTpe)
    if (ctorSchemaExpr.isEmpty) {
      c.abort(
        c.enclosingPosition,
        s"Unable to summon GolemSchema for constructor type $ctorType on ${traitSymbol.fullName}.$schemaHint"
      )
    }

    c.Expr[AgentImplementationType[Trait, Ctor]](
      q"""
      val metadata = $metadataExpr
      _root_.golem.runtime.AgentImplementationType[$traitType, $ctorType](
        metadata = metadata,
        idSchema = $ctorSchemaExpr,
        buildInstance = { val f = ($build).asInstanceOf[$ctorType => $traitType]; (input: $ctorType, _: _root_.golem.Principal) => f(input) },
        methods = $methodsExpr
      ).asInstanceOf[_root_.golem.runtime.AgentImplementationType[$traitType, ${weakTypeOf[Ctor]}]]
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

    val inputSchemaExpr = accessMode match {
      case ParamAccessMode.MultiArgs =>
        multiParamSchemaExpr(c)(methodName, nonPrincipalParams)
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

    val handlerExpr = buildHandler(c)(traitType, method, allParams, nonPrincipalParams, accessMode, inputType, isAsync)

    if (isAsync) {
      q"""
        _root_.golem.runtime.AsyncImplementationMethod[$traitType, $inputType, $outputType](
          metadata = $methodMetadataExpr,
          inputSchema = $inputSchemaExpr,
          outputSchema = $outputSchemaInstance,
          handler = $handlerExpr
        )
      """
    } else {
      q"""
        _root_.golem.runtime.SyncImplementationMethod[$traitType, $inputType, $outputType](
          metadata = $methodMetadataExpr,
          inputSchema = $inputSchemaExpr,
          outputSchema = $outputSchemaInstance,
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

  private def multiParamSchemaExpr(c: blackbox.Context)(
    methodName: String,
    params: List[(c.universe.TermName, c.universe.Type)]
  ): c.Tree = {
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
      new _root_.golem.data.GolemSchema[List[Any]] {
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

        override def encode(value: List[Any]): Either[String, _root_.golem.data.StructuredValue] = {
          val values = value.toVector
          if (values.length != params.length)
            Left("Parameter count mismatch for method '" + $methodName + "'. Expected " + $expectedCount + ", found " + values.length)
          else {
            val builder = List.newBuilder[_root_.golem.data.NamedElementValue]
            var idx = 0
            var error: Option[String] = None
            while (idx < params.length && error.isEmpty) {
              val (paramName, codec) = params(idx)
              codec.encodeElement(values(idx)) match {
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

        override def decode(value: _root_.golem.data.StructuredValue): Either[String, List[Any]] =
          value match {
            case _root_.golem.data.StructuredValue.Tuple(elements) =>
              if (elements.length != params.length)
                Left("Structured element count mismatch for method '" + $methodName + "'. Expected " + $expectedCount + ", found " + elements.length)
              else {
                val builder = List.newBuilder[Any]
                var idx = 0
                var error: Option[String] = None
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
                        builder += decoded
                    }
                  }
                  idx += 1
                }
                error.fold[Either[String, List[Any]]](Right(builder.result()))(Left(_))
              }
            case other =>
              Left("Structured value mismatch for method '" + $methodName + "'. Expected tuple payload, found: " + other)
          }

        override def elementSchema: _root_.golem.data.ElementSchema =
          throw new UnsupportedOperationException("Multi-param schema cannot be used as a single element")

        override def encodeElement(value: List[Any]): Either[String, _root_.golem.data.ElementValue] =
          Left("Multi-param schema cannot be encoded as a single element")

        override def decodeElement(value: _root_.golem.data.ElementValue): Either[String, List[Any]] =
          Left("Multi-param schema cannot be decoded from a single element")
      }
    """
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
    params: List[(c.universe.TermName, c.universe.Type)]
  ): c.universe.Type = {
    import c.universe._
    accessMode match {
      case ParamAccessMode.NoArgs    => typeOf[Unit]
      case ParamAccessMode.SingleArg => params.head._2
      case ParamAccessMode.MultiArgs => typeOf[List[Any]]
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

    // Extract constructor type from Id class on the trait
    val expectedCtor: Type = {
      val idAnnotationType = typeOf[golem.runtime.annotations.id]

      val annotatedClass = traitType.members.collectFirst {
        case sym if sym.isClass && !sym.isMethod &&
          sym.annotations.exists(ann => ann.tree.tpe != null && ann.tree.tpe =:= idAnnotationType) =>
          sym
      }

      val constructorClass = annotatedClass.orElse {
        val byName = traitType.member(TypeName("Id"))
        if (byName == NoSymbol) None else Some(byName)
      }.getOrElse {
        c.abort(c.enclosingPosition,
          s"Agent trait ${traitSymbol.fullName} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters.")
      }
      val primaryCtor = constructorClass.asClass.primaryConstructor.asMethod
      val params = primaryCtor.paramLists.flatten.filter(_.isTerm).map(_.typeSignature)
      params match {
        case Nil      => typeOf[Unit]
        case p :: Nil => p
        case ps       =>
          val tupleClass = rootMirror.staticClass(s"scala.Tuple${ps.length}")
          appliedType(tupleClass.toType, ps)
      }
    }

    if (expectedCtor =:= typeOf[Unit]) {
      if (identityParams.nonEmpty) {
        c.abort(
          c.enclosingPosition,
          s"Trait ${traitSymbol.fullName} has an empty Id class (Unit constructor), " +
            s"but Impl ${implSymbol.fullName} has ${identityParams.length} non-Config constructor parameter(s): " +
            s"${identityParams.map(_.name.toString).mkString(", ")}"
        )
      }
    } else {
      if (identityParams.length == 1) {
        if (!(identityParams.head.tpe =:= expectedCtor)) {
          c.abort(
            c.enclosingPosition,
            s"Constructor parameter '${identityParams.head.name}' has type ${identityParams.head.tpe}, " +
              s"but Id class expects $expectedCtor"
          )
        }
      } else if (identityParams.length > 1) {
        val tuplePrefix = "scala.Tuple"
        if (expectedCtor.typeSymbol.fullName.startsWith(tuplePrefix)) {
          val tupleArgs = expectedCtor.typeArgs
          if (tupleArgs.length != identityParams.length) {
            c.abort(
              c.enclosingPosition,
              s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
                s"Id class expects a ${tupleArgs.length}-element tuple"
            )
          }
          identityParams.zip(tupleArgs).foreach { case (param, expected) =>
            if (!(param.tpe =:= expected)) {
              c.abort(
                c.enclosingPosition,
                s"Constructor parameter '${param.name}' has type ${param.tpe}, " +
                  s"expected $expected (from Id class parameters)"
              )
            }
          }
        } else {
          c.abort(
            c.enclosingPosition,
            s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
              s"Id class type $expectedCtor is not a tuple type"
          )
        }
      }
    }

    // Determine the Ctor type based on identity params
    val ctorType: Type = identityParams match {
      case Nil      => expectedCtor
      case p :: Nil => p.tpe
      case ps       =>
        val types = ps.map(_.tpe)
        val tupleClass = rootMirror.staticClass(s"scala.Tuple${types.length}")
        appliedType(tupleClass.toType, types)
    }

    val metadataExpr = q"_root_.golem.runtime.macros.AgentDefinitionMacro.generate[$traitType]"
    val methodsExpr  = buildImplementationMethodsExpr(c)(traitType, metadataExpr)

    val ctorSchemaTpe  = appliedType(typeOf[GolemSchema[_]].typeConstructor, ctorType)
    val ctorSchemaExpr = c.inferImplicitValue(ctorSchemaTpe)
    if (ctorSchemaExpr.isEmpty) {
      c.abort(
        c.enclosingPosition,
        s"Unable to summon GolemSchema for constructor type $ctorType on ${traitSymbol.fullName}.$schemaHint"
      )
    }

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
        // Detect if trait extends AgentConfig[X] and summon ConfigBuilder
        val agentConfigBases = traitType.baseClasses.filter(_.fullName == "golem.config.AgentConfig")
        if (agentConfigBases.isEmpty) {
          q"_root_.scala.None"
        } else {
          val configTypes = agentConfigBases.flatMap { sym =>
            traitType.baseType(sym) match {
              case TypeRef(_, _, List(arg)) => Some(arg)
              case _                       => None
            }
          }
          configTypes.headOption match {
            case Some(configType) =>
              val cbTpe = appliedType(typeOf[ConfigBuilder[_]].typeConstructor, configType)
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

    // Build the instance construction lambda: (Ctor, Principal) => Trait
    val inputTermName    = TermName("input")
    val principalArgName = TermName("principalArg")

    val buildInstanceExpr: Tree = configParam match {
      case None =>
        val argTerms: List[Tree] = paramInfos.map { pi =>
          if (pi.isPrincipal) {
            q"$principalArgName"
          } else {
            identityParams match {
              case Nil      => c.abort(c.enclosingPosition, "Unexpected: no identity params but trying to construct args")
              case _ :: Nil => q"$inputTermName"
              case ps       =>
                val idx = ps.indexWhere(_.index == pi.index)
                q"$inputTermName.${TermName(s"_${idx + 1}")}"
            }
          }
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
          } else {
            identityParams match {
              case Nil      => c.abort(c.enclosingPosition, "Unexpected: identity param not found")
              case _ :: Nil => q"$inputTermName"
              case ps       =>
                val idx = ps.indexWhere(_.index == pi.index)
                q"$inputTermName.${TermName(s"_${idx + 1}")}"
            }
          }
        }
        q"($inputTermName: $ctorType, $principalArgName: _root_.golem.Principal) => new $implType(..$argTerms)"
    }

    c.Expr[AgentImplementationType[Trait, Any]](
      q"""
      {
        val metadata = $metadataExpr
        _root_.golem.runtime.AgentImplementationType[$traitType, $ctorType](
          metadata = metadata,
          idSchema = $ctorSchemaExpr,
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
