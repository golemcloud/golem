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
import golem.runtime.{
  AgentImplementationType,
  AgentMetadata,
  AsyncImplementationMethod,
  ImplementationMethod,
  MethodMetadata,
  SnapshotHandlers,
  SnapshotPayload,
  Snapshotting,
  SyncImplementationMethod
}
import scala.quoted.*

object AgentImplementationMacro {
  private val schemaHint: String =
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  inline def implementationType[Trait](inline build: => Trait): AgentImplementationType[Trait, Unit] =
    ${ implementationTypeImpl[Trait]('build) }

  inline def implementationTypeWithCtor[Trait, Ctor](
    inline build: Ctor => Trait
  ): AgentImplementationType[Trait, Ctor] =
    ${ implementationTypeWithCtorImpl[Trait, Ctor]('build) }

  inline def implementationTypeFromClass[Trait, Impl <: Trait]: golem.runtime.AgentImplementationType[Trait, ?] =
    ${ implementationTypeFromClassImpl[Trait, Impl] }

  private def implementationTypeFromClassImpl[Trait: Type, Impl: Type](using
    Quotes
  ): Expr[AgentImplementationType[Trait, ?]] = {
    import quotes.reflect.*

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"@agentImplementation target must be a trait, found: ${traitSymbol.fullName}")

    val implRepr   = TypeRepr.of[Impl]
    val implSymbol = implRepr.typeSymbol

    if implSymbol.flags.is(Flags.Abstract) then
      report.errorAndAbort(s"Impl type must be a concrete class, found abstract: ${implSymbol.fullName}")
    if implSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"Impl type must be a concrete class, found trait: ${implSymbol.fullName}")
    if implSymbol.flags.is(Flags.Module) then
      report.errorAndAbort(s"Impl type must be a concrete class, found object: ${implSymbol.fullName}")

    val implConstructor = implSymbol.primaryConstructor
    if implConstructor == Symbol.noSymbol then
      report.errorAndAbort(s"Impl type ${implSymbol.fullName} has no accessible primary constructor")

    val termParamLists = implConstructor.paramSymss.filter(_.forall(_.isTerm))
    if termParamLists.length != 1 then
      report.errorAndAbort(
        s"Impl type ${implSymbol.fullName} must have exactly one term parameter list, found ${termParamLists.length}"
      )

    val params: List[(String, TypeRepr)] = termParamLists.head.map { sym =>
      sym.tree match {
        case v: ValDef => (sym.name, v.tpt.tpe)
        case other     => report.errorAndAbort(s"Unsupported parameter declaration in ${implSymbol.fullName}: $other")
      }
    }

    val configFullName    = "golem.config.Config"
    val principalFullName = "golem.Principal"

    case class ParamInfo(
      name: String,
      tpe: TypeRepr,
      index: Int,
      isConfig: Boolean,
      isPrincipal: Boolean,
      configInnerType: Option[TypeRepr]
    )

    val paramInfos: List[ParamInfo] = params.zipWithIndex.map { case ((name, tpe), idx) =>
      tpe.dealias match {
        case AppliedType(tycon, List(inner)) if tycon.typeSymbol.fullName == configFullName =>
          ParamInfo(name, tpe, idx, isConfig = true, isPrincipal = false, configInnerType = Some(inner))
        case t if t.typeSymbol.fullName == principalFullName =>
          ParamInfo(name, tpe, idx, isConfig = false, isPrincipal = true, configInnerType = None)
        case _ =>
          ParamInfo(name, tpe, idx, isConfig = false, isPrincipal = false, configInnerType = None)
      }
    }

    val configParams    = paramInfos.filter(_.isConfig)
    val principalParams = paramInfos.filter(_.isPrincipal)
    val identityParams  = paramInfos.filter(pi => !pi.isConfig && !pi.isPrincipal)

    if configParams.length > 1 then
      report.errorAndAbort(
        s"Impl type ${implSymbol.fullName} has ${configParams.length} Config[_] parameters, at most one is allowed"
      )

    if principalParams.length > 1 then
      report.errorAndAbort(
        s"Impl type ${implSymbol.fullName} has ${principalParams.length} Principal parameters, at most one is allowed"
      )

    val expectedCtor = agentInputTypeRepr[Trait]

    if expectedCtor =:= TypeRepr.of[Unit] then {
      if identityParams.nonEmpty then
        report.errorAndAbort(
          s"Trait ${traitSymbol.fullName} has an empty Id class (Unit constructor), " +
            s"but Impl ${implSymbol.fullName} has ${identityParams.length} non-Config constructor parameter(s): " +
            s"${identityParams.map(_.name).mkString(", ")}"
        )
    } else {
      if identityParams.length == 1 then {
        if !(identityParams.head.tpe =:= expectedCtor) then
          report.errorAndAbort(
            s"Constructor parameter '${identityParams.head.name}' has type ${identityParams.head.tpe.show}, " +
              s"but Id class expects ${expectedCtor.show}"
          )
      } else if identityParams.length > 1 then {
        expectedCtor match {
          case AppliedType(tycon, tupleArgs) if tycon.typeSymbol.fullName.startsWith("scala.Tuple") =>
            if tupleArgs.length != identityParams.length then
              report.errorAndAbort(
                s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
                  s"Id class expects a ${tupleArgs.length}-element tuple"
              )
            identityParams.zip(tupleArgs).foreach { case (param, expected) =>
              if !(param.tpe =:= expected) then
                report.errorAndAbort(
                  s"Constructor parameter '${param.name}' has type ${param.tpe.show}, " +
                    s"expected ${expected.show} (from Id class parameters)"
                )
            }
          case _ =>
            report.errorAndAbort(
              s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
                s"Id class type ${expectedCtor.show} is not a tuple type"
            )
        }
      }
      // identityParams.isEmpty is valid (config-only constructor on a non-Unit Id class)
    }

    // Determine the Ctor type based on identity params
    val ctorTypeRepr: TypeRepr = identityParams match {
      case Nil      => expectedCtor
      case p :: Nil => p.tpe
      case ps       =>
        val types      = ps.map(_.tpe)
        val tupleClass = Symbol.requiredClass(s"scala.Tuple${types.length}")
        tupleClass.typeRef.appliedTo(types)
    }

    ctorTypeRepr.asType match {
      case '[ctor] =>
        val metadataExpr  = '{ AgentDefinitionMacro.generate[Trait] }
        val methodSymbols = traitSymbol.methodMembers.collect {
          case method
              if method.owner == traitSymbol && method.flags.is(
                Flags.Deferred
              ) && method.isDefDef =>
            method
        }
        val methodsExpr = buildImplementationMethodsExpr[Trait](methodSymbols, metadataExpr)

        val ctorSchemaExpr =
          Expr.summon[GolemSchema[ctor]].getOrElse {
            report.errorAndAbort(
              s"Unable to summon GolemSchema for constructor type ${ctorTypeRepr.show} on ${traitSymbol.fullName}.$schemaHint"
            )
          }

        val configParam = configParams.headOption

        // Validate config param against AgentConfig[X] on the trait
        val configBuilderExpr: Expr[Option[ConfigBuilder[_]]] = configParam match {
          case Some(cp) =>
            val configInner      = cp.configInnerType.get
            val agentConfigBases = traitRepr.baseClasses.filter(_.fullName == "golem.config.AgentConfig")
            if agentConfigBases.isEmpty then
              report.errorAndAbort(
                s"Impl ${implSymbol.fullName} has a Config[${configInner.show}] parameter, " +
                  s"but trait ${traitSymbol.fullName} does not extend AgentConfig"
              )

            val configTypes = agentConfigBases.flatMap { sym =>
              traitRepr.baseType(sym) match {
                case AppliedType(_, List(arg)) => Some(arg)
                case _                         => None
              }
            }

            configTypes.headOption match {
              case Some(agentConfigType) =>
                if !(configInner =:= agentConfigType) then
                  report.errorAndAbort(
                    s"Config parameter type Config[${configInner.show}] does not match " +
                      s"AgentConfig[${agentConfigType.show}] on trait ${traitSymbol.fullName}"
                  )
                configInner.asType match {
                  case '[t] =>
                    Expr.summon[ConfigBuilder[t]] match {
                      case Some(builderExpr) =>
                        '{ Some($builderExpr: ConfigBuilder[_]) }
                      case None =>
                        report.errorAndAbort(
                          s"No implicit ConfigBuilder available for config type ${Type.show[t]}.\n" +
                            "Hint: Add an implicit Schema[T] for your config type, which provides ConfigBuilder automatically."
                        )
                    }
                }
              case None =>
                report.errorAndAbort(
                  s"Trait ${traitSymbol.fullName} extends AgentConfig but type argument could not be extracted"
                )
            }

          case None =>
            detectConfigBuilder[Trait]
        }

        val hasPrincipalParam = principalParams.nonEmpty

        // Build the instance construction lambda: (Ctor, Principal) => Trait
        val buildInstanceExpr: Expr[(ctor, golem.Principal) => Trait] = configParam match {
          case None =>
            // No config param - straightforward construction
            val lambdaType =
              MethodType(List("input", "principal"))(
                _ => List(ctorTypeRepr, TypeRepr.of[golem.Principal]),
                _ => TypeRepr.of[Trait]
              )

            Lambda(
              Symbol.spliceOwner,
              lambdaType,
              { (_, lambdaParams) =>
                val inputTerm            = lambdaParams.head.asInstanceOf[Term]
                val principalTerm        = lambdaParams(1).asInstanceOf[Term]
                val argTerms: List[Term] = paramInfos.map { pi =>
                  if pi.isPrincipal then principalTerm
                  else {
                    identityParams match {
                      case Nil      => report.errorAndAbort("Unexpected: no identity params but trying to construct args")
                      case _ :: Nil => inputTerm
                      case ps       =>
                        val idx = ps.indexWhere(_.index == pi.index)
                        Select.unique(inputTerm, s"_${idx + 1}")
                    }
                  }
                }
                Apply(Select(New(TypeTree.of[Impl]), implConstructor), argTerms).asExprOf[Trait].asTerm
              }
            ).asExprOf[(ctor, golem.Principal) => Trait]

          case Some(cp) =>
            val configInner = cp.configInnerType.get
            configInner.asType match {
              case '[configT] =>
                val builderExpr = Expr.summon[ConfigBuilder[configT]].get
                val lambdaType  =
                  MethodType(List("input", "principal"))(
                    _ => List(ctorTypeRepr, TypeRepr.of[golem.Principal]),
                    _ => TypeRepr.of[Trait]
                  )

                Lambda(
                  Symbol.spliceOwner,
                  lambdaType,
                  { (_, lambdaParams) =>
                    val inputTerm     = lambdaParams.head.asInstanceOf[Term]
                    val principalTerm = lambdaParams(1).asInstanceOf[Term]

                    // Generate: _root_.golem.config.ConfigLoader.createLazyConfig(builder)
                    // ConfigLoader is in core/js, not available in macros, so we construct the call via reflection
                    val configLoaderModule     = Symbol.requiredModule("golem.config.ConfigLoader")
                    val createLazyConfigMethod = configLoaderModule.methodMember("createLazyConfig").head
                    val configTerm             = Apply(
                      TypeApply(
                        Select(Ref(configLoaderModule), createLazyConfigMethod),
                        List(TypeTree.of[configT])
                      ),
                      List(builderExpr.asTerm)
                    )

                    val argTerms: List[Term] = paramInfos.map { pi =>
                      if pi.isConfig then configTerm
                      else if pi.isPrincipal then principalTerm
                      else {
                        identityParams match {
                          case Nil      => report.errorAndAbort("Unexpected: identity param not found")
                          case _ :: Nil => inputTerm
                          case ps       =>
                            val idx = ps.indexWhere(_.index == pi.index)
                            Select.unique(inputTerm, s"_${idx + 1}")
                        }
                      }
                    }

                    Apply(Select(New(TypeTree.of[Impl]), implConstructor), argTerms).asExprOf[Trait].asTerm
                  }
                ).asExprOf[(ctor, golem.Principal) => Trait]
            }
        }

        val snapshotHandlersExpr: Expr[Option[SnapshotHandlers[Trait]]] = {
          val customHooks         = detectCustomSnapshotHooks(implSymbol)
          val snapshottedState    = detectSnapshottedStateType(implRepr)
          val snapshotting        = extractSnapshottingFromTrait(traitSymbol)
          val snapshottingEnabled = snapshotting match {
            case Snapshotting.Enabled(_) => true
            case _                       => false
          }

          customHooks match {
            case Some((saveSym, loadSym)) =>
              // Use helper methods to avoid Scala 3 LambdaLift issues with
              // macro-generated lambdas that capture outer lambda parameters.

              // Build raw save: (Trait) => Future[Array[Byte]]
              val rawSaveLambdaExpr: Expr[Trait => scala.concurrent.Future[Array[Byte]]] = {
                val lambdaType = MethodType(List("instance"))(
                  _ => List(TypeRepr.of[Trait]),
                  _ => TypeRepr.of[scala.concurrent.Future[Array[Byte]]]
                )
                Lambda(
                  Symbol.spliceOwner,
                  lambdaType,
                  { (_, params) =>
                    val instanceTerm = params.head.asInstanceOf[Term]
                    val implTerm     = TypeApply(
                      Select.unique(instanceTerm, "asInstanceOf"),
                      List(TypeTree.of[Impl])
                    )
                    Apply(Select(implTerm, saveSym), Nil)
                  }
                ).asExprOf[Trait => scala.concurrent.Future[Array[Byte]]]
              }

              // Build raw load: (Trait, Array[Byte]) => Future[Unit]
              val rawLoadLambdaExpr: Expr[(Trait, Array[Byte]) => scala.concurrent.Future[Unit]] = {
                val lambdaType = MethodType(List("instance", "bytes"))(
                  _ => List(TypeRepr.of[Trait], TypeRepr.of[Array[Byte]]),
                  _ => TypeRepr.of[scala.concurrent.Future[Unit]]
                )
                Lambda(
                  Symbol.spliceOwner,
                  lambdaType,
                  { (_, params) =>
                    val instanceTerm = params.head.asInstanceOf[Term]
                    val bytesTerm    = params(1).asInstanceOf[Term]
                    val implTerm     = TypeApply(
                      Select.unique(instanceTerm, "asInstanceOf"),
                      List(TypeTree.of[Impl])
                    )
                    Apply(Select(implTerm, loadSym), List(bytesTerm))
                  }
                ).asExprOf[(Trait, Array[Byte]) => scala.concurrent.Future[Unit]]
              }

              // Wrap via helper methods that handle the .map(...) at runtime
              val saveLambdaExpr = '{ SnapshotHandlers.wrapSave[Trait]($rawSaveLambdaExpr) }
              val loadLambdaExpr = '{ SnapshotHandlers.wrapLoad[Trait]($rawLoadLambdaExpr) }
              '{
                Some(
                  SnapshotHandlers[Trait](
                    save = $saveLambdaExpr,
                    load = $loadLambdaExpr
                  )
                )
              }
            case None =>
              snapshottedState match {
                case Some(stateTpe) =>
                  stateTpe.asType match {
                    case '[s] =>
                      '{
                        Some(
                          SnapshotHandlers[Trait](
                            save = (instance: Trait) => {
                              val snap  = instance.asInstanceOf[golem.Snapshotted[s]]
                              val codec = snap.stateSchema.derive(zio.blocks.schema.json.JsonCodecDeriver)
                              scala.concurrent.Future.successful(
                                SnapshotPayload(
                                  bytes = codec.encode(snap.state),
                                  mimeType = "application/json"
                                )
                              )
                            },
                            load = (instance: Trait, bytes: Array[Byte]) => {
                              val snap  = instance.asInstanceOf[golem.Snapshotted[s]]
                              val codec = snap.stateSchema.derive(zio.blocks.schema.json.JsonCodecDeriver)
                              codec.decode(bytes) match {
                                case Right(restored) =>
                                  snap.state = restored
                                  scala.concurrent.Future.successful(instance)
                                case Left(err) =>
                                  scala.concurrent.Future.failed(
                                    new IllegalArgumentException(
                                      s"Failed to decode JSON snapshot for ${${ Expr(implSymbol.fullName) }}: " + err
                                    )
                                  )
                              }
                            }
                          )
                        )
                      }
                  }
                case None =>
                  if (snapshottingEnabled) {
                    report.errorAndAbort(
                      s"Snapshotting is enabled for ${traitSymbol.fullName}, but ${implSymbol.fullName} " +
                        s"provides no snapshot support. Either:\n" +
                        s"  (1) Mix in Snapshotted[S] and implement `stateSchema` with your Schema[S] instance\n" +
                        s"  (2) Implement `def saveSnapshot(): Future[Array[Byte]]` and `def loadSnapshot(bytes: Array[Byte]): Future[Unit]`"
                    )
                  }
                  '{ None }
              }
          }
        }

        '{
          val metadata = $metadataExpr
          AgentImplementationType[Trait, ctor](
            metadata = metadata,
            idSchema = $ctorSchemaExpr,
            buildInstance = (input: ctor, principal: golem.Principal) => $buildInstanceExpr(input, principal),
            methods = $methodsExpr,
            configBuilder = $configBuilderExpr,
            configInjectedViaConstructor = ${ Expr(configParam.isDefined) },
            principalInjectedViaConstructor = ${ Expr(hasPrincipalParam) },
            snapshotHandlers = $snapshotHandlersExpr
          )
        }
    }
  }

  private def implementationTypeImpl[Trait: Type](
    buildExpr: Expr[Trait]
  )(using Quotes): Expr[AgentImplementationType[Trait, Unit]] = {
    import quotes.reflect.*

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"@agentImplementation target must be a trait, found: ${traitSymbol.fullName}")

    val methodSymbols = traitSymbol.methodMembers.collect {
      case method if method.owner == traitSymbol && method.flags.is(Flags.Deferred) && method.isDefDef =>
        method
    }

    val metadataExpr = '{ AgentDefinitionMacro.generate[Trait] }
    val methodsExpr  = buildImplementationMethodsExpr[Trait](methodSymbols, metadataExpr)

    val ctorSchemaExpr =
      Expr.summon[GolemSchema[Unit]].getOrElse {
        report.errorAndAbort(
          s"Unable to summon GolemSchema for Unit constructor type on ${traitSymbol.fullName}.$schemaHint"
        )
      }

    val configBuilderExpr = detectConfigBuilder[Trait]

    '{
      val metadata = $metadataExpr
      AgentImplementationType[Trait, Unit](
        metadata = metadata,
        idSchema = $ctorSchemaExpr,
        buildInstance = (_: Unit, _: golem.Principal) => $buildExpr,
        methods = $methodsExpr,
        configBuilder = $configBuilderExpr
      )
    }
  }

  private def implementationTypeWithCtorImpl[Trait: Type, Ctor: Type](
    buildExpr: Expr[Any]
  )(using Quotes): Expr[AgentImplementationType[Trait, Ctor]] = {
    import quotes.reflect.*

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"@agentImplementation target must be a trait, found: ${traitSymbol.fullName}")

    val expectedCtor = agentInputTypeRepr[Trait]
    val gotCtor      = TypeRepr.of[Ctor]
    if !(gotCtor =:= expectedCtor) then
      report.errorAndAbort(
        s"Constructor function must have input type matching Id class parameters (${expectedCtor.show}) on ${traitSymbol.fullName} (found: ${gotCtor.show})"
      )

    val metadataExpr = '{ AgentDefinitionMacro.generate[Trait] }
    val methodsExpr  = buildImplementationMethodsExpr[Trait](
      traitSymbol.methodMembers.collect {
        case method if method.owner == traitSymbol && method.flags.is(Flags.Deferred) && method.isDefDef =>
          method
      },
      metadataExpr
    )

    val ctorSchemaExpr =
      Expr.summon[GolemSchema[Ctor]].getOrElse {
        report.errorAndAbort(
          s"Unable to summon GolemSchema for constructor type ${Type.show[Ctor]} on ${traitSymbol.fullName}.$schemaHint"
        )
      }

    val buildTyped = buildExpr.asExprOf[Ctor => Trait]

    val configBuilderExpr = detectConfigBuilder[Trait]

    '{
      val metadata = $metadataExpr
      AgentImplementationType[Trait, Ctor](
        metadata = metadata,
        idSchema = $ctorSchemaExpr,
        buildInstance = (input: Ctor, _: golem.Principal) => $buildTyped(input),
        methods = $methodsExpr,
        configBuilder = $configBuilderExpr
      )
    }
  }

  private def extractSnapshottingFromTrait(using Quotes)(traitSymbol: quotes.reflect.Symbol): Snapshotting = {
    import quotes.reflect.*

    val snapStr = traitSymbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        args.collectFirst { case NamedArg("snapshotting", Literal(StringConstant(v))) =>
          v
        }.orElse {
          args.lift(7).collect { case Literal(StringConstant(v)) => v }
        }
    }.flatten.getOrElse("disabled")

    Snapshotting.parse(snapStr).getOrElse(Snapshotting.Disabled)
  }

  private def detectCustomSnapshotHooks(using
    Quotes
  )(
    implSymbol: quotes.reflect.Symbol
  ): Option[(quotes.reflect.Symbol, quotes.reflect.Symbol)] = {
    import quotes.reflect.*

    val saveDecls = implSymbol.declaredMethod("saveSnapshot")
    val loadDecls = implSymbol.declaredMethod("loadSnapshot")

    val saveMatches = saveDecls.filter { sym =>
      sym.isDefDef &&
      !sym.flags.is(Flags.Private) &&
      !sym.flags.is(Flags.Protected) &&
      sym.paramSymss.flatten.filter(_.isTerm).isEmpty &&
      {
        sym.tree match {
          case d: DefDef =>
            val retType = d.returnTpt.tpe.dealias
            retType.typeSymbol.fullName == "scala.concurrent.Future"
          case _ => false
        }
      }
    }

    val loadMatches = loadDecls.filter { sym =>
      sym.isDefDef &&
      !sym.flags.is(Flags.Private) &&
      !sym.flags.is(Flags.Protected) &&
      {
        val termParams = sym.paramSymss.flatten.filter(_.isTerm)
        termParams.length == 1 && {
          termParams.head.tree match {
            case v: ValDef => v.tpt.tpe.dealias =:= TypeRepr.of[Array[Byte]]
            case _         => false
          }
        }
      } &&
      {
        sym.tree match {
          case d: DefDef =>
            val retType = d.returnTpt.tpe.dealias
            retType.typeSymbol.fullName == "scala.concurrent.Future"
          case _ => false
        }
      }
    }

    if (saveMatches.nonEmpty != loadMatches.nonEmpty)
      report.errorAndAbort(
        s"${implSymbol.fullName} must declare both saveSnapshot and loadSnapshot, or neither"
      )

    saveMatches.headOption.zip(loadMatches.headOption).headOption
  }

  private def detectSnapshottedStateType(using
    Quotes
  )(
    implRepr: quotes.reflect.TypeRepr
  ): Option[quotes.reflect.TypeRepr] = {
    import quotes.reflect.*

    val snapSym = Symbol.requiredClass("golem.Snapshotted")

    if (!implRepr.baseClasses.contains(snapSym)) None
    else
      implRepr.baseType(snapSym).dealias match {
        case AppliedType(_, List(stateTpe)) => Some(stateTpe)
        case _                              => None
      }
  }

  private def agentInputTypeRepr[Trait: Type](using Quotes): quotes.reflect.TypeRepr = {
    import quotes.reflect.*
    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    val idFQN = "golem.runtime.annotations.id"

    def hasIdAnnotation(sym: Symbol): Boolean =
      sym.annotations.exists {
        case Apply(Select(New(tpt), _), _) => tpt.tpe.dealias.typeSymbol.fullName == idFQN
        case _                             => false
      }

    val constructorClass = traitSymbol.declarations.find { sym =>
      sym.isClassDef && hasIdAnnotation(sym)
    }.orElse {
      traitSymbol.declarations.find { sym =>
        sym.isClassDef && sym.name == "Id"
      }
    }

    constructorClass match {
      case None =>
        report.errorAndAbort(
          s"Agent trait ${traitSymbol.name} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
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

  private def detectConfigBuilder[Trait: Type](using Quotes): Expr[Option[ConfigBuilder[_]]] = {
    import quotes.reflect.*

    val traitRepr        = TypeRepr.of[Trait]
    val agentConfigBases = traitRepr.baseClasses.filter(_.fullName == "golem.config.AgentConfig")

    if (agentConfigBases.isEmpty) '{ None }
    else {
      val configTypes = agentConfigBases.flatMap { sym =>
        traitRepr.baseType(sym) match {
          case AppliedType(_, List(arg)) => Some(arg)
          case _                         => None
        }
      }

      configTypes.headOption match {
        case Some(configType) =>
          configType.asType match {
            case '[t] =>
              Expr.summon[ConfigBuilder[t]] match {
                case Some(builderExpr) =>
                  '{ Some($builderExpr: ConfigBuilder[_]) }
                case None =>
                  report.errorAndAbort(
                    s"No implicit ConfigBuilder available for config type ${Type.show[t]}.\n" +
                      "Hint: Add an implicit Schema[T] for your config type, which provides ConfigBuilder automatically."
                  )
              }
          }
        case None => '{ None }
      }
    }
  }

  private def buildImplementationMethodsExpr[Trait: Type](using
    quotes: Quotes
  )(
    methods: List[quotes.reflect.Symbol],
    metadataExpr: Expr[AgentMetadata]
  ): Expr[List[ImplementationMethod[Trait]]] = {
    import quotes.reflect.*

    val principalFullName = "golem.Principal"

    val methodExprs: List[Expr[ImplementationMethod[Trait]]] = methods.map { methodSymbol =>
      val methodName       = methodSymbol.name
      val methodMetadata   = methodMetadataExpr(metadataExpr, methodName)
      val allParameters    = extractParameters(methodSymbol)
      val parameterDetails = allParameters.filter { case (_, tpe) =>
        tpe.dealias.typeSymbol.fullName != principalFullName
      }

      val accessMode: MethodParamAccess =
        parameterDetails match {
          case Nil      => MethodParamAccess.NoArgs
          case _ :: Nil => MethodParamAccess.SingleArg
          case _        => MethodParamAccess.MultiArgs
        }

      val inputTypeRepr =
        accessMode match {
          case MethodParamAccess.NoArgs    => TypeRepr.of[Unit]
          case MethodParamAccess.SingleArg => parameterDetails.head._2
          case MethodParamAccess.MultiArgs => TypeRepr.of[List[Any]]
        }

      val (isAsync, payloadTpe, handlerTpe) = methodReturnInfo(methodSymbol)

      val methodImpl: Expr[ImplementationMethod[Trait]] =
        inputTypeRepr.asType match {
          case '[in] =>
            payloadTpe.asType match {
              case '[out] =>
                val inputSchemaExpr =
                  accessMode match {
                    case MethodParamAccess.MultiArgs =>
                      multiParamSchemaExpr(methodName, parameterDetails).asExprOf[GolemSchema[in]]
                    case _ =>
                      summonSchema[in](methodName, "input")
                  }
                val outputSchemaExpr = summonSchema[out](methodName, "output")

                if !isAsync then {
                  val handlerExpr =
                    handlerLambda[Trait, in, out](methodSymbol, accessMode, parameterDetails, allParameters)
                  '{
                    val metadataEntry = $methodMetadata
                    SyncImplementationMethod[Trait, in, out](
                      metadata = metadataEntry,
                      inputSchema = $inputSchemaExpr,
                      outputSchema = $outputSchemaExpr,
                      handler = $handlerExpr
                    )
                  }
                } else
                  handlerTpe.asType match {
                    case '[handlerReturn] =>
                      val handlerExpr =
                        handlerLambda[Trait, in, handlerReturn](
                          methodSymbol,
                          accessMode,
                          parameterDetails,
                          allParameters
                        )
                      val normalized =
                        handlerExpr.asExprOf[(Trait, in, golem.Principal) => scala.concurrent.Future[out]]
                      '{
                        val metadataEntry = $methodMetadata
                        AsyncImplementationMethod[Trait, in, out](
                          metadata = metadataEntry,
                          inputSchema = $inputSchemaExpr,
                          outputSchema = $outputSchemaExpr,
                          handler = $normalized
                        )
                      }
                    case _ =>
                      report.errorAndAbort(s"Unsupported async handler type for method $methodName")
                  }
              case _ =>
                report.errorAndAbort(s"Unsupported output type for method $methodName")
            }
          case _ =>
            report.errorAndAbort(s"Unsupported input type for method $methodName")
        }

      methodImpl
    }

    Expr.ofList(methodExprs)
  }

  private def methodMetadataExpr(using
    Quotes
  )(
    metadataExpr: Expr[AgentMetadata],
    methodName: String
  ): Expr[MethodMetadata] =
    '{
      $metadataExpr.methods.find(_.name == ${ Expr(methodName) }).getOrElse {
        throw new IllegalStateException(s"Method metadata missing for ${${ Expr(methodName) }}")
      }
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

  private def methodReturnInfo(using
    Quotes
  )(
    method: quotes.reflect.Symbol
  ): (Boolean, quotes.reflect.TypeRepr, quotes.reflect.TypeRepr) = {
    import quotes.reflect.*

    method.tree match {
      case d: DefDef =>
        val returnType = d.returnTpt.tpe
        asyncInnerType(returnType) match {
          case Some(inner) =>
            (true, inner, returnType)
          case None =>
            (false, returnType, returnType)
        }
      case other =>
        report.errorAndAbort(s"Unable to read return type for ${method.name}: $other")
    }
  }

  private def asyncInnerType(using
    Quotes
  )(
    tpe: quotes.reflect.TypeRepr
  ): Option[quotes.reflect.TypeRepr] = {
    import quotes.reflect.*

    tpe match {
      case AppliedType(constructor, args) if constructor.typeSymbol.fullName == "scala.concurrent.Future" =>
        args.headOption
      case _ =>
        None
    }
  }

  private enum MethodParamAccess {
    case NoArgs
    case SingleArg
    case MultiArgs
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
  ): Expr[GolemSchema[List[Any]]] = {

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
      new GolemSchema[List[Any]] {
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

        override def encode(value: List[Any]): Either[String, _root_.golem.data.StructuredValue] = {
          val values = value.toVector
          if (values.length != params.length)
            Left(
              s"Parameter count mismatch for method '${$methodNameExpr}'. Expected ${$expectedCountExpr}, found $${values.length}"
            )
          else {
            val builder = List.newBuilder[_root_.golem.data.NamedElementValue]
            var idx     = 0
            while (idx < params.length) {
              val (paramName, codec) = params(idx)
              codec.encodeElement(values(idx)) match {
                case Left(err) =>
                  return Left(s"Failed to encode parameter '$paramName' in method '${$methodNameExpr}': $err")
                case Right(elementValue) =>
                  builder += _root_.golem.data.NamedElementValue(paramName, elementValue)
              }
              idx += 1
            }
            Right(_root_.golem.data.StructuredValue.Tuple(builder.result()))
          }
        }

        override def decode(
          value: _root_.golem.data.StructuredValue
        ): Either[String, List[Any]] =
          value match {
            case _root_.golem.data.StructuredValue.Tuple(elements) =>
              if (elements.length != params.length)
                Left(
                  s"Structured element count mismatch for method '${$methodNameExpr}'. Expected ${$expectedCountExpr}, found $${elements.length}"
                )
              else {
                val builder = List.newBuilder[Any]
                var idx     = 0
                while (idx < params.length) {
                  val (paramName, codec) = params(idx)
                  val element            = elements(idx)
                  if (element.name != paramName)
                    return Left(
                      s"Structured element name mismatch for method '${$methodNameExpr}'. Expected '$$paramName', found '${element.name}'"
                    )
                  codec.decodeElement(element.value) match {
                    case Left(err) =>
                      return Left(s"Failed to decode parameter '$paramName' in method '${$methodNameExpr}': $err")
                    case Right(decoded) =>
                      builder += decoded
                  }
                  idx += 1
                }
                Right(builder.result())
              }
            case other =>
              Left(s"Structured value mismatch for method '${$methodNameExpr}'. Expected tuple payload, found: $other")
          }

        override def elementSchema: _root_.golem.data.ElementSchema =
          throw new UnsupportedOperationException("Multi-param schema cannot be used as a single element")

        override def encodeElement(value: List[Any]): Either[String, _root_.golem.data.ElementValue] =
          Left("Multi-param schema cannot be encoded as a single element")

        override def decodeElement(value: _root_.golem.data.ElementValue): Either[String, List[Any]] =
          Left("Multi-param schema cannot be decoded from a single element")
      }
    }
  }

  private def handlerLambda[Trait: Type, In: Type, Out: Type](using
    quotes: Quotes
  )(
    method: quotes.reflect.Symbol,
    access: MethodParamAccess,
    parameters: List[(String, quotes.reflect.TypeRepr)],
    allParameters: List[(String, quotes.reflect.TypeRepr)]
  ): Expr[(Trait, In, golem.Principal) => Out] = {
    import quotes.reflect.*

    val principalFullName = "golem.Principal"

    val lambdaType =
      MethodType(List("instance", "input", "principal"))(
        _ => List(TypeRepr.of[Trait], TypeRepr.of[In], TypeRepr.of[golem.Principal]),
        _ => TypeRepr.of[Out]
      )

    Lambda(
      Symbol.spliceOwner,
      lambdaType,
      { (lambdaOwner, params) =>
        val instanceTerm  = params.head.asInstanceOf[Term]
        val inputTerm     = params(1).asInstanceOf[Term]
        val principalTerm = params(2).asInstanceOf[Term]

        val callTerm: Term = access match {
          case MethodParamAccess.NoArgs =>
            val argTerms = allParameters.map { case (_, paramType) =>
              if paramType.dealias.typeSymbol.fullName == principalFullName then principalTerm
              else report.errorAndAbort(s"Unexpected non-principal param in NoArgs method ${method.name}")
            }
            if argTerms.isEmpty then Apply(Select(instanceTerm, method), Nil)
            else Apply(Select(instanceTerm, method), argTerms)
          case MethodParamAccess.SingleArg =>
            val argTerms = allParameters.map { case (_, paramType) =>
              if paramType.dealias.typeSymbol.fullName == principalFullName then principalTerm
              else inputTerm
            }
            Apply(Select(instanceTerm, method), argTerms)
          case MethodParamAccess.MultiArgs =>
            val valuesSym =
              Symbol.newVal(lambdaOwner, "values", TypeRepr.of[List[Any]], Flags.EmptyFlags, Symbol.noSymbol)
            val valuesVal         = ValDef(valuesSym, Some(inputTerm))
            val valuesRef         = Ref(valuesSym).asExprOf[List[Any]]
            val expectedCount     = parameters.length
            val lengthCheck: Term = {
              val expectedExpr          = Expr(expectedCount)
              val methodLabel           = Expr(method.name)
              val checkExpr: Expr[Unit] =
                '{
                  if ($valuesRef.length != $expectedExpr)
                    throw new IllegalArgumentException(
                      s"Parameter count mismatch when invoking method '${$methodLabel}'. Expected ${$expectedExpr}."
                    )
                }
              checkExpr.asTerm
            }
            var nonPrincipalIdx      = 0
            val argTerms: List[Term] = allParameters.map { case (_, paramType) =>
              if paramType.dealias.typeSymbol.fullName == principalFullName then principalTerm
              else {
                val idx = nonPrincipalIdx
                nonPrincipalIdx += 1
                paramType.asType match {
                  case '[p] =>
                    '{ $valuesRef.apply(${ Expr(idx) }).asInstanceOf[p] }.asTerm
                }
              }
            }
            Block(
              List(valuesVal),
              Block(
                List(lengthCheck),
                Apply(Select(instanceTerm, method), argTerms)
              )
            )
        }

        callTerm
      }
    ).asExprOf[(Trait, In, golem.Principal) => Out]
  }
}
