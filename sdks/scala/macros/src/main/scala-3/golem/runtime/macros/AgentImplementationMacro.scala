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
import golem.runtime.{
  AgentImplementationType,
  AgentMetadata,
  AsyncImplementationMethod,
  ImplementationMethod,
  InputRecordCodec,
  MethodMetadata,
  OutputCodec,
  ParamCodec,
  SnapshotHandlers,
  SnapshotPayload,
  Snapshotting,
  SyncImplementationMethod
}
import golem.schema.{FromSchema, IntoSchema}
import scala.quoted.*

object AgentImplementationMacro {
  private val schemaHint: String =
    "\nHint: IntoSchema/FromSchema are derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Use `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n"
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

    // The user-supplied Id-class params (Principal filtered out) are the source
    // of truth for the constructor input record; validate the impl's identity
    // params against them.
    val idParams = agentInputParams[Trait]

    idParams match {
      case Nil =>
        if identityParams.nonEmpty then
          report.errorAndAbort(
            s"Trait ${traitSymbol.fullName} has an empty Id class (Unit constructor), " +
              s"but Impl ${implSymbol.fullName} has ${identityParams.length} non-Config constructor parameter(s): " +
              s"${identityParams.map(_.name).mkString(", ")}"
          )
      case (_, expected) :: Nil =>
        if identityParams.length == 1 then {
          if !(identityParams.head.tpe =:= expected) then
            report.errorAndAbort(
              s"Constructor parameter '${identityParams.head.name}' has type ${identityParams.head.tpe.show}, " +
                s"but Id class expects ${expected.show}"
            )
        } else if identityParams.length > 1 then
          report.errorAndAbort(
            s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
              s"Id class declares a single constructor parameter"
          )
      // identityParams.isEmpty is valid (config-only constructor on a non-Unit Id class)
      case multi =>
        if identityParams.nonEmpty then {
          if multi.length != identityParams.length then
            report.errorAndAbort(
              s"Impl ${implSymbol.fullName} has ${identityParams.length} identity params but " +
                s"Id class declares ${multi.length} constructor parameter(s)"
            )
          identityParams.zip(multi).foreach { case (param, (_, expected)) =>
            if !(param.tpe =:= expected) then
              report.errorAndAbort(
                s"Constructor parameter '${param.name}' has type ${param.tpe.show}, " +
                  s"expected ${expected.show} (from Id class parameters)"
              )
          }
        }
      // identityParams.isEmpty is valid (config-only constructor on a non-Unit Id class)
    }

    // Determine the Ctor type + wire access mode from the Id-class params (the
    // source of truth for the constructor input record). Multi-param ctors are
    // represented positionally as `Vector[Any]`, matching method inputs.
    val ctorAccess: MethodParamAccess = idParams match {
      case Nil      => MethodParamAccess.NoArgs
      case _ :: Nil => MethodParamAccess.SingleArg
      case _        => MethodParamAccess.MultiArgs
    }
    val ctorTypeRepr: TypeRepr = ctorAccess match {
      case MethodParamAccess.NoArgs    => TypeRepr.of[Unit]
      case MethodParamAccess.SingleArg => idParams.head._2
      case MethodParamAccess.MultiArgs => TypeRepr.of[Vector[Any]]
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

        val ctorCodecExpr =
          inputCodecExpr[ctor](ctorAccess, s"constructor of ${traitSymbol.fullName}", idParams)

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
                        pi.tpe.asType match {
                          case '[p] =>
                            '{ ${ inputTerm.asExprOf[Vector[Any]] }.apply(${ Expr(idx) }).asInstanceOf[p] }.asTerm
                        }
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
                            pi.tpe.asType match {
                              case '[p] =>
                                '{ ${ inputTerm.asExprOf[Vector[Any]] }.apply(${ Expr(idx) }).asInstanceOf[p] }.asTerm
                            }
                        }
                      }
                    }

                    Apply(Select(New(TypeTree.of[Impl]), implConstructor), argTerms).asExprOf[Trait].asTerm
                  }
                ).asExprOf[(ctor, golem.Principal) => Trait]
            }
        }

        val snapshotHandlersExpr: Expr[Option[SnapshotHandlers[Trait]]] = {
          val snapshottedState    = detectSnapshottedStateType(implRepr)
          val customHooks         = detectCustomSnapshotHooks(implSymbol, snapshottedState.isDefined)
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

              val loadLambdaExpr: Expr[
                (Array[Byte], golem.runtime.SnapshotRestoreContext) => scala.concurrent.Future[Trait]
              ] = {
                val lambdaType = MethodType(List("bytes", "context"))(
                  _ => List(TypeRepr.of[Array[Byte]], TypeRepr.of[golem.runtime.SnapshotRestoreContext]),
                  _ => TypeRepr.of[scala.concurrent.Future[Trait]]
                )
                Lambda(
                  Symbol.spliceOwner,
                  lambdaType,
                  { (_, params) =>
                    val bytesTerm   = params.head.asInstanceOf[Term]
                    val contextTerm = params(1).asInstanceOf[Term]
                    Apply(Select(Ref(implSymbol.companionModule), loadSym), List(bytesTerm, contextTerm))
                  }
                ).asExprOf[
                  (Array[Byte], golem.runtime.SnapshotRestoreContext) => scala.concurrent.Future[Trait]
                ]
              }

              val saveLambdaExpr = '{ SnapshotHandlers.wrapSave[Trait]($rawSaveLambdaExpr) }
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
                      val schemaExpr = Expr.summon[zio.blocks.schema.Schema[s]].getOrElse {
                        report.errorAndAbort(
                          s"Automatic snapshotting for ${implSymbol.fullName} requires an implicit Schema[${stateTpe.show}]"
                        )
                      }
                      val loadSym = detectSnapshottedLoadHook(implSymbol, stateTpe)
                      '{
                        Some(
                          SnapshotHandlers[Trait](
                            save = (instance: Trait) => {
                              val snap  = instance.asInstanceOf[golem.Snapshotted[s]]
                              val codec = $schemaExpr.derive(zio.blocks.schema.json.JsonCodecDeriver)
                              scala.concurrent.Future.successful(
                                SnapshotPayload(
                                  bytes = codec.encode(snap.state),
                                  mimeType = "application/json"
                                )
                              )
                            },
                            load = (bytes: Array[Byte], context: golem.runtime.SnapshotRestoreContext) => {
                              val codec = $schemaExpr.derive(zio.blocks.schema.json.JsonCodecDeriver)
                              codec.decode(bytes) match {
                                case Right(restored) =>
                                  ${
                                    Apply(
                                      Select(Ref(implSymbol.companionModule), loadSym),
                                      List('restored.asTerm, 'context.asTerm)
                                    ).asExprOf[scala.concurrent.Future[Trait]]
                                  }
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
                        s"  (1) Mix in Snapshotted[S], provide Schema[S], and declare companion `loadSnapshot(state: S, context: SnapshotRestoreContext): Future[Impl]`\n" +
                        s"  (2) Implement instance `saveSnapshot(): Future[Array[Byte]]` and companion `loadSnapshot(bytes: Array[Byte], context: SnapshotRestoreContext): Future[Impl]`"
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
            ctorCodec = $ctorCodecExpr,
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

    val configBuilderExpr = detectConfigBuilder[Trait]

    '{
      val metadata = $metadataExpr
      AgentImplementationType[Trait, Unit](
        metadata = metadata,
        ctorCodec = InputRecordCodec.unit,
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

    val idParams = agentInputParams[Trait]
    val gotCtor  = TypeRepr.of[Ctor]

    val ctorCodecExpr: Expr[InputRecordCodec[Ctor]] =
      idParams match {
        case Nil =>
          if !(gotCtor =:= TypeRepr.of[Unit]) then
            report.errorAndAbort(
              s"Constructor function input must be Unit for the empty Id class on ${traitSymbol.fullName} (found: ${gotCtor.show})"
            )
          '{ InputRecordCodec.unit }.asExprOf[InputRecordCodec[Ctor]]
        case (name, expected) :: Nil =>
          if !(gotCtor =:= expected) then
            report.errorAndAbort(
              s"Constructor function input must match the Id class parameter (${expected.show}) on ${traitSymbol.fullName} (found: ${gotCtor.show})"
            )
          val into = summonInto[Ctor](s"constructor of ${traitSymbol.fullName}")
          val from = summonFrom[Ctor](s"constructor of ${traitSymbol.fullName}")
          '{ InputRecordCodec.single[Ctor](${ Expr(name) })($into, $from) }
        case _ =>
          report.errorAndAbort(
            s"implementationType[Trait, Ctor] does not support multi-parameter constructors on " +
              s"${traitSymbol.fullName}. Use `implementationTypeFromClass` (or a single-field Id class) instead."
          )
      }

    val metadataExpr = '{ AgentDefinitionMacro.generate[Trait] }
    val methodsExpr  = buildImplementationMethodsExpr[Trait](
      traitSymbol.methodMembers.collect {
        case method if method.owner == traitSymbol && method.flags.is(Flags.Deferred) && method.isDefDef =>
          method
      },
      metadataExpr
    )

    val buildTyped = buildExpr.asExprOf[Ctor => Trait]

    val configBuilderExpr = detectConfigBuilder[Trait]

    '{
      val metadata = $metadataExpr
      AgentImplementationType[Trait, Ctor](
        metadata = metadata,
        ctorCodec = $ctorCodecExpr,
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
    implSymbol: quotes.reflect.Symbol,
    isSnapshotted: Boolean
  ): Option[(quotes.reflect.Symbol, quotes.reflect.Symbol)] = {
    import quotes.reflect.*

    val saveDecls = implSymbol.declaredMethod("saveSnapshot")
    val loadDecls = implSymbol.companionModule.declaredMethod("loadSnapshot")

    def isPublicNonGeneric(sym: Symbol): Boolean =
      sym.isDefDef &&
        !sym.flags.is(Flags.Private) &&
        !sym.flags.is(Flags.Protected) &&
        !sym.paramSymss.exists(_.exists(_.isType))

    def termParameterLists(sym: Symbol): List[List[Symbol]] =
      sym.paramSymss.filter(_.forall(_.isTerm))

    def returnsFutureOf(sym: Symbol, expected: TypeRepr): Boolean =
      sym.tree match {
        case d: DefDef =>
          d.returnTpt.tpe.dealias match {
            case AppliedType(future, List(result)) =>
              future.typeSymbol.fullName == "scala.concurrent.Future" && result =:= expected
            case _ => false
          }
        case _ => false
      }

    val saveMatches = saveDecls.filter { sym =>
      isPublicNonGeneric(sym) &&
      termParameterLists(sym) == List(Nil) &&
      returnsFutureOf(sym, TypeRepr.of[Array[Byte]])
    }

    val loadMatches = loadDecls.filter { sym =>
      isPublicNonGeneric(sym) &&
      (termParameterLists(sym) match {
        case List(termParams) if termParams.length == 2 =>
          val types = termParams.map(_.tree.asInstanceOf[ValDef].tpt.tpe.dealias)
          types.head =:= TypeRepr.of[Array[Byte]] &&
          types(1) =:= TypeRepr.of[golem.runtime.SnapshotRestoreContext]
        case _ => false
      }) &&
      returnsFutureOf(sym, implSymbol.typeRef)
    }

    val hasDeclarations = saveDecls.nonEmpty || (!isSnapshotted && loadDecls.nonEmpty)
    if (hasDeclarations && (saveMatches.size != 1 || loadMatches.size != 1))
      report.errorAndAbort(
        s"${implSymbol.fullName} must declare exactly instance saveSnapshot(): Future[Array[Byte]] and companion " +
          s"loadSnapshot(bytes: Array[Byte], context: SnapshotRestoreContext): Future[${implSymbol.name}], with no type parameters or additional parameter lists"
      )

    saveMatches.headOption.zip(loadMatches.headOption).headOption
  }

  private def detectSnapshottedLoadHook(using
    Quotes
  )(
    implSymbol: quotes.reflect.Symbol,
    stateType: quotes.reflect.TypeRepr
  ): quotes.reflect.Symbol = {
    import quotes.reflect.*

    val declarations = implSymbol.companionModule.declaredMethod("loadSnapshot")
    val matches      = declarations.filter { sym =>
      val termParameterLists = sym.paramSymss.filter(_.forall(_.isTerm))
      val exactParameters    = termParameterLists match {
        case List(params) if params.length == 2 =>
          val types = params.map(_.tree.asInstanceOf[ValDef].tpt.tpe.dealias)
          types.head =:= stateType && types(1) =:= TypeRepr.of[golem.runtime.SnapshotRestoreContext]
        case _ => false
      }
      val exactResult = sym.tree match {
        case d: DefDef =>
          d.returnTpt.tpe.dealias match {
            case AppliedType(future, List(result)) =>
              future.typeSymbol.fullName == "scala.concurrent.Future" && result =:= implSymbol.typeRef
            case _ => false
          }
        case _ => false
      }
      sym.isDefDef &&
      !sym.flags.is(Flags.Private) &&
      !sym.flags.is(Flags.Protected) &&
      !sym.paramSymss.exists(_.exists(_.isType)) &&
      exactParameters &&
      exactResult
    }

    if (matches.size != 1) {
      report.errorAndAbort(
        s"${implSymbol.fullName} mixes in Snapshotted[${stateType.show}] and must declare exactly public companion " +
          s"loadSnapshot(state: ${stateType.show}, context: SnapshotRestoreContext): Future[${implSymbol.name}], with no type parameters or additional parameter lists"
      )
    }
    matches.head
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

  /**
   * The user-supplied `class Id(...)` parameters (name + type), Principal
   * params filtered out. These define the constructor input record's shape,
   * matching `AgentDefinitionMacro`'s `ConstructorMetadata`.
   */
  private def agentInputParams[Trait: Type](using Quotes): List[(String, quotes.reflect.TypeRepr)] = {
    import quotes.reflect.*
    val traitSymbol = TypeRepr.of[Trait].typeSymbol

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
        classSym.primaryConstructor.paramSymss.flatten.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef => (sym.name, v.tpt.tpe)
              case other     => report.errorAndAbort(s"Unsupported parameter declaration in Id class: $other")
            }
        }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != "golem.Principal" }
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
          case MethodParamAccess.MultiArgs => TypeRepr.of[Vector[Any]]
        }

      val (isAsync, payloadTpe, handlerTpe) = methodReturnInfo(methodSymbol)

      val methodImpl: Expr[ImplementationMethod[Trait]] =
        inputTypeRepr.asType match {
          case '[in] =>
            payloadTpe.asType match {
              case '[out] =>
                val inputCodec  = inputCodecExpr[in](accessMode, s"method $methodName", parameterDetails)
                val outputCodec = outputCodecExpr[out](s"method $methodName")

                if !isAsync then {
                  val handlerExpr =
                    handlerLambda[Trait, in, out](methodSymbol, accessMode, parameterDetails, allParameters)
                  '{
                    val metadataEntry = $methodMetadata
                    SyncImplementationMethod[Trait, in, out](
                      metadata = metadataEntry,
                      inputCodec = $inputCodec,
                      outputCodec = $outputCodec,
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
                          inputCodec = $inputCodec,
                          outputCodec = $outputCodec,
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
              Symbol.newVal(lambdaOwner, "values", TypeRepr.of[Vector[Any]], Flags.EmptyFlags, Symbol.noSymbol)
            val valuesVal         = ValDef(valuesSym, Some(inputTerm))
            val valuesRef         = Ref(valuesSym).asExprOf[Vector[Any]]
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
