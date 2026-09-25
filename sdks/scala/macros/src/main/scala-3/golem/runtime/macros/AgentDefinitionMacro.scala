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

import golem.config.{AgentConfigDeclaration, AgentConfigSource, ConfigSchema, Secret}
import golem.runtime.annotations.{description, prompt, readOnly}
import golem.runtime.{
  AgentMetadata,
  AgentTypeKind,
  CachePolicy,
  ConstructorMetadata,
  FieldSource,
  InputMetadata,
  MethodMetadata,
  OutputMetadata,
  ParameterMetadata,
  ReadOnlyConfig,
  Snapshotting,
  SnapshottingConfig,
  WireAgentMetadata
}
import golem.schema.{IntoSchema, SchemaGraph}
import golem.runtime.http.{
  DurableStreamRouteLoadOptions,
  DurableStreamRouteOptions,
  DurableStreamSlotOptions,
  DurableStreamSlotSource,
  HttpAgentValidation,
  HeaderVariable,
  HttpEndpointDetails,
  HttpMethod,
  HttpMountDetails,
  HttpRouteParser,
  HttpValidation,
  PathSegment,
  QueryVariable
}

import scala.quoted.*

object AgentDefinitionMacro {
  private final case class EndpointSelector(method: String, path: String)
  private final case class StreamSlot(
    selector: EndpointSelector,
    source: String,
    slot: String,
    name: Option[String],
    contentType: Option[String]
  )
  private final case class StreamOptions(
    selector: EndpointSelector,
    writes: Option[Boolean],
    streamDelete: Option[Boolean],
    invocationDelete: Option[Boolean],
    readers: Option[Int],
    appends: Option[Int]
  )
  private val schemaHint: String =
    "\nHint: IntoSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Use `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n"
  inline def generate[T]: AgentMetadata         = ${ impl[T] }
  inline def generateWire[T]: WireAgentMetadata = ${ wireImpl[T] }

  private def wireImpl[T: Type](using Quotes): Expr[WireAgentMetadata] = {
    import quotes.reflect.*
    val core      = new ToolMacroCore
    val compiled  = new CompiledWireMetadata[core.type](core)
    val traitType = TypeRepr.of[T]
    val sym       = traitType.typeSymbol
    if (!sym.flags.is(Flags.Trait)) report.errorAndAbort(s"@agent target must be a trait, found: ${sym.fullName}")
    val router = HttpDeclarationMacro.isRouter(sym)
    val hasAgentDefinition =
      sym.annotations.exists(_.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition")
    if (router && hasAgentDefinition) report.errorAndAbort("Use either @httpRouter or @agentDefinition, not both")
    if (!router && !hasAgentDefinition)
      report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${sym.fullName}")
    val name             = if (router) HttpDeclarationMacro.string(sym, "typeName", 0) else agentDefinitionTypeName(sym).getOrElse(sym.name)
    val descriptionValue = annotationString(sym, TypeRepr.of[description]).orElse(docstringText(sym))
    val mode             = if (router) Some("ephemeral") else agentDefinitionMode(sym).map(_.valueOrAbort)
    val mount            = if (router) Some(HttpDeclarationMacro.routerMountValue(sym)) else extractHttpMount(sym, name)

    def graph(tpe: TypeRepr): SchemaGraph = tpe.asType match {
      case '[a] => compiled.graph(core.q.reflect.TypeRepr.of[a])
    }
    def params(method: Symbol): List[(String, TypeRepr)] = method.paramSymss.flatten.filter(_.isTerm).map { p =>
      p.name -> p.tree.asInstanceOf[ValDef].tpt.tpe
    }
    def isPrincipal(tpe: TypeRepr): Boolean                    = tpe.dealias.typeSymbol.fullName == "golem.Principal"
    def input(values: List[(String, TypeRepr)]): InputMetadata = InputMetadata(values.collect {
      case (name, tpe) if !isPrincipal(tpe) => ParameterMetadata(name, FieldSource.UserSupplied, graph(tpe))
    })
    val (identity, constructorDescription) = if (router) {
      if (sym.declarations.exists(c => c.isClassDef && (c.name == "Id" || HttpDeclarationMacro.has(c, "id"))))
        report.errorAndAbort("router-constructor: HTTP routers cannot declare constructor parameters")
      (Nil, descriptionValue.getOrElse(name))
    } else {
      val idClass = sym.declarations
        .find(c => c.isClassDef && HttpDeclarationMacro.has(c, "id"))
        .orElse(sym.declarations.find(c => c.isClassDef && c.name == "Id"))
        .getOrElse(
          report.errorAndAbort(
            s"Agent trait ${sym.name} must define a `class Id(...)` to declare its constructor parameters."
          )
        )
      (params(idClass.primaryConstructor), docstringText(idClass).getOrElse(descriptionValue.getOrElse(name)))
    }
    val constructor = ConstructorMetadata(
      None,
      constructorDescription,
      None,
      input(identity)
    )
    val methods = sym.methodMembers.collect {
      case method if method.flags.is(Flags.Deferred) && method.isDefDef =>
        val handler  = HttpDeclarationMacro.has(method, "httpHandler")
        val provider = HttpDeclarationMacro.has(method, "openApiProvider")
        if ((handler || provider) && !router) report.errorAndAbort("HTTP roles require @httpRouter")
        if (router && (handler == provider))
          report.errorAndAbort("router-method-role: each method must have exactly one HTTP role")
        val arguments      = params(method)
        val principalNames = arguments.collect { case (name, tpe) if isPrincipal(tpe) => name }.toSet
        val headers        = extractHeaderVars(method)
        if (router && (HttpDeclarationMacro.has(method, "endpoint") || headers.nonEmpty))
          report.errorAndAbort("handler-endpoint-policy: router policy belongs to the mount")
        if (provider && arguments.nonEmpty)
          report.errorAndAbort("provider-schema: provider must be parameterless")
        validateEndpoints(method, name, mount.isDefined, arguments.map(_._1).toSet, principalNames, headers)
        val readonly = extractReadOnly(method, principalNames.nonEmpty)
        if (mode.contains("ephemeral") && readonly.nonEmpty)
          report.errorAndAbort(s"Agent '$name' is ephemeral but method '${method.name}' is marked with @readOnly.")
        val out = unwrapAsyncType(method.tree.asInstanceOf[DefDef].returnTpt.tpe)
        MethodMetadata(
          method.name,
          annotationString(method, TypeRepr.of[description]).orElse(docstringText(method)),
          annotationString(method, TypeRepr.of[prompt]),
          None,
          input(arguments),
          if (out =:= TypeRepr.of[Unit]) OutputMetadata.Unit else OutputMetadata.Single(graph(out)),
          if (handler) List(HttpEndpointDetails(HttpMethod.Any, Nil, Nil, Nil, None, None)) else extractEndpoints(method, headers),
          readonly
        )
    }
    def config(tpe: TypeRepr, path: List[String]): List[AgentConfigDeclaration] = tpe.dealias.asType match {
      case '[Secret[a]] =>
        val inner = graph(TypeRepr.of[a])
        List(
          AgentConfigDeclaration(
            AgentConfigSource.Secret,
            path,
            inner.copy(root =
              golem.schema.SchemaType(golem.schema.SchemaTypeBody.SecretType(golem.schema.SecretSpec(inner.root)))
            )
          )
        )
      case _ if tpe.typeSymbol.caseFields.nonEmpty && !(tpe <:< TypeRepr.of[Tuple]) =>
        tpe.typeSymbol.caseFields.flatMap(f => config(tpe.memberType(f), path :+ f.name))
      case _ => List(AgentConfigDeclaration(AgentConfigSource.Local, path, graph(tpe)))
    }
    val configTypes = traitType.baseClasses.filter(_.fullName == "golem.config.AgentConfig").flatMap { base =>
      traitType.baseType(base).typeArgs
    }
    if (configTypes.size > 1) report.errorAndAbort("Agent trait may extend at most one AgentConfig[T]")
    val snapshotting = if (router) Snapshotting.Disabled else Snapshotting
      .parse(extractAgentDefinitionStringArg(sym, "snapshotting", 7).getOrElse("disabled"))
      .fold(error => report.errorAndAbort(error), value => value)
    val metadata = AgentMetadata(
      name,
      if (router) AgentTypeKind.HttpRouter else AgentTypeKind.Regular,
      descriptionValue,
      mode,
      methods,
      constructor,
      mount,
      configTypes.flatMap(tpe => config(tpe, Nil)),
      snapshotting
    )
    mount.foreach { m =>
      HttpValidation
        .validateMountVarsAreNotPrincipal(name, m, identity.collect { case (n, tpe) if isPrincipal(tpe) => n }.toSet)
        .left
        .foreach(error => report.errorAndAbort(error))
    }
    try HttpValidation.validateHttpMountFromMetadata(metadata)
    catch { case error: IllegalArgumentException => report.errorAndAbort(error.getMessage) }
    HttpAgentValidation.validate(metadata).left.foreach(error => report.errorAndAbort(error))
    compiled.literal(WireAgentMetadata.fromModel(metadata))
  }

  private def impl[T: Type](using Quotes): Expr[AgentMetadata] = {
    import quotes.reflect.*

    val typeRepr   = TypeRepr.of[T]
    val typeSymbol = typeRepr.typeSymbol

    if !typeSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"@agent target must be a trait, found: ${typeSymbol.fullName}")

    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name

    val router             = HttpDeclarationMacro.isRouter(typeSymbol)
    val hasAgentDefinition =
      typeSymbol.annotations.exists {
        case Apply(Select(New(tpt), _), _)
            if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
          true
        case _ => false
      }

    if (router && hasAgentDefinition) report.errorAndAbort("Use either @httpRouter or @agentDefinition, not both")
    val agentTypeName =
      if (router) HttpDeclarationMacro.string(typeSymbol, "typeName", 0)
      else
        agentDefinitionTypeName(typeSymbol).map(validateTypeName).getOrElse {
          if !hasAgentDefinition then
            report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${typeSymbol.fullName}")
          defaultTypeNameFromTrait(typeSymbol)
        }

    val traitDescription = annotationString(typeSymbol, TypeRepr.of[description]).orElse(docstringText(typeSymbol))
    // Note: `@agentDefinition` has a default `mode = Durable`. We omit that default in metadata via
    // `agentDefinitionMode`.
    val traitMode = if (router) Some(Expr("ephemeral")) else agentDefinitionMode(typeSymbol)

    // --- HTTP mount extraction from @agentDefinition ---
    val httpMountExpr: Expr[Option[HttpMountDetails]] =
      if (router) HttpDeclarationMacro.routerMount(typeSymbol) else literal(extractHttpMount(typeSymbol, agentTypeName))
    val hasMount =
      router || extractAgentDefinitionStringArg(typeSymbol, "mount", positionalIndex = 2).exists(_.nonEmpty)
    val isEphemeral = router || agentDefinitionModeString(typeSymbol).contains("ephemeral")

    // Ephemeral + @readOnly is not allowed.
    if (isEphemeral) {
      typeSymbol.methodMembers.foreach { method =>
        if (method.flags.is(Flags.Deferred) && method.isDefDef && hasReadOnlyAnnotation(method)) {
          report.errorAndAbort(
            s"Agent '$agentTypeName' is ephemeral but method '${method.name}' is marked with @readOnly. " +
              s"Read-only methods have no effect on ephemeral agents (no shared state to read). " +
              s"Remove the @readOnly annotation or change the agent mode to Durable."
          )
        }
      }
    }

    val methods = typeSymbol.methodMembers.collect {
      case method if method.flags.is(Flags.Deferred) && method.isDefDef =>
        val handler  = HttpDeclarationMacro.has(method, "httpHandler")
        val provider = HttpDeclarationMacro.has(method, "openApiProvider")
        if ((handler || provider) && !router) report.errorAndAbort("HTTP roles require @httpRouter")
        if (router && (handler == provider))
          report.errorAndAbort("router-method-role: each method must have exactly one HTTP role")
        if (router && (HttpDeclarationMacro.has(method, "endpoint") || extractHeaderVars(method).nonEmpty))
          report.errorAndAbort("handler-endpoint-policy: router policy belongs to the mount")
        if (provider && method.paramSymss.flatten.exists(_.isTerm))
          report.errorAndAbort("provider-schema: provider must be parameterless")
        methodMetadata(method, agentTypeName, hasMount)
    }

    val ctorDescription = traitDescription.getOrElse(agentTypeName)
    val idSchema        = inferIdSchema(typeRepr, Expr(ctorDescription))

    // --- Mount-level Principal validation ---
    if (hasMount) {
      val mountPath         = extractAgentDefinitionStringArg(typeSymbol, "mount", positionalIndex = 2).getOrElse("")
      val mountSegments     = HttpRouteParser.parsePathOnly(mountPath, "mount").getOrElse(Nil)
      val idPrincipalParams = idConstructorPrincipalParams(typeRepr)
      if (idPrincipalParams.nonEmpty) {
        if (HttpDeclarationMacro.exposesFiles(typeSymbol))
          report.errorAndAbort("filesystem-constructor: caller-dependent identities cannot expose files")
        val mount = HttpMountDetails(mountSegments, false, false, Nil, Nil, Nil, Nil, None)
        HttpValidation.validateMountVarsAreNotPrincipal(agentTypeName, mount, idPrincipalParams) match {
          case Left(err) => report.errorAndAbort(err)
          case Right(()) => ()
        }
      }
    }

    val configExpr: Expr[List[AgentConfigDeclaration]] = detectAgentConfig(typeRepr).getOrElse('{ Nil })

    val snapshottingStr =
      extractAgentDefinitionStringArg(typeSymbol, "snapshotting", positionalIndex = 7).getOrElse("disabled")
    val snapshottingValue: Snapshotting = Snapshotting.parse(snapshottingStr) match {
      case Right(v)  => v
      case Left(err) =>
        report.errorAndAbort(s"Invalid snapshotting on @agentDefinition for ${typeSymbol.fullName}: $err")
    }
    val snapshottingExpr: Expr[Snapshotting] = snapshottingValue match {
      case Snapshotting.Disabled                                    => '{ Snapshotting.Disabled }
      case Snapshotting.Enabled(SnapshottingConfig.Default)         => '{ Snapshotting.Enabled(SnapshottingConfig.Default) }
      case Snapshotting.Enabled(SnapshottingConfig.Periodic(nanos)) =>
        '{ Snapshotting.Enabled(SnapshottingConfig.Periodic(${ Expr(nanos) })) }
      case Snapshotting.Enabled(SnapshottingConfig.EveryN(count)) =>
        '{ Snapshotting.Enabled(SnapshottingConfig.EveryN(${ Expr(count) })) }
    }

    val kindExpr = if (router) '{ AgentTypeKind.HttpRouter } else '{ AgentTypeKind.Regular }
    '{
      HttpAgentValidation.checked(
        AgentMetadata(
          name = ${
            Expr(agentTypeName)
          },
          kind = $kindExpr,
          description = ${
            optionalString(traitDescription)
          },
          mode = ${
            optionalExprString(traitMode)
          },
          methods = ${
            Expr.ofList(methods)
          },
          constructor = $idSchema,
          httpMount = $httpMountExpr,
          config = $configExpr,
          snapshotting = $snapshottingExpr
        )
      )
    }
  }

  private def agentDefinitionTypeName(using
    Quotes
  )(symbol: quotes.reflect.Symbol): Option[String] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        args.collectFirst {
          case Literal(StringConstant(value))                       => value
          case NamedArg("typeName", Literal(StringConstant(value))) => value
        }
    }.flatten.map(_.trim).filter(_.nonEmpty)
  }

  private def validateTypeName(value: String): String =
    value

  /**
   * Read the `mode` argument of `@agentDefinition` as a plain wire string at
   * compile time so we can drive compile-time validations (e.g. ephemeral +
   * read-only).
   */
  private def agentDefinitionModeString(using
    Quotes
  )(symbol: quotes.reflect.Symbol): Option[String] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        val rawModeArg: Option[Term] =
          args.collectFirst { case NamedArg("mode", arg: Term) => arg }.orElse {
            args.lift(1).collect { case t: Term => t }
          }
        rawModeArg.flatMap { term =>
          def loop(t: Term): Option[String] = t match {
            case Inlined(_, _, inner: Term) => loop(inner)
            case _                          =>
              t.symbol.name match {
                case "$lessinit$greater$default$2" => Some("durable")
                case "Durable"                     => Some("durable")
                case "Ephemeral"                   => Some("ephemeral")
                case _                             => None
              }
          }
          loop(term)
        }
    }.flatten
  }

  private def hasReadOnlyAnnotation(using
    Quotes
  )(method: quotes.reflect.Symbol): Boolean = {
    import quotes.reflect.*
    method.annotations.exists {
      case Apply(Select(New(tpt), _), _)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.readOnly" =>
        true
      case _ => false
    }
  }

  private def agentDefinitionMode(using
    Quotes
  )(symbol: quotes.reflect.Symbol): Option[Expr[String]] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        // The compiler may represent default args as:
        // - positional args (typeName, mode)
        // - NamedArg(...) entries (even if not explicitly provided by the user)
        // Be robust and accept both forms.
        val rawModeArg: Option[Term] =
          args.collectFirst { case NamedArg("mode", arg: Term) => arg }.orElse {
            args.lift(1).collect { case t: Term => t }
          }

        rawModeArg.flatMap {
          case Literal(StringConstant(value)) =>
            // (Legacy) allow stringly-typed values.
            // Note: annotation defaults may be inlined by the compiler; treat default "durable" as unset.
            val v = value.trim.toLowerCase
            if (v == "durable") None else Some(Expr(v))
          case term: Term =>
            // Treat default Durable as unset to preserve the "omit defaults" metadata behavior.
            val e = durabilityWireExpr(term)
            if (e.valueOrAbort == "durable") None else Some(e)
        }
    }.flatten
  }

  private def methodMetadata(using
    Quotes
  )(method: quotes.reflect.Symbol, agentName: String, hasMount: Boolean): Expr[MethodMetadata] = {
    import quotes.reflect.*

    val methodName   = method.name
    val descExpr     = optionalString(annotationString(method, TypeRepr.of[description]).orElse(docstringText(method)))
    val promptExpr   = optionalString(annotationString(method, TypeRepr.of[prompt]))
    val inputSchema  = methodInputSchema(method)
    val outputSchema = methodOutputSchema(method)

    // --- HTTP endpoint extraction ---
    val headerVars      = extractHeaderVars(method)
    val endpointDetails =
      if (HttpDeclarationMacro.has(method, "httpHandler"))
        List(HttpEndpointDetails(HttpMethod.Any, Nil, Nil, Nil, None, None))
      else extractEndpoints(method, headerVars)
    val endpointListExpr = literal(endpointDetails)

    // --- Compile-time validation ---
    val principalFullName   = "golem.Principal"
    val allTermParams       = method.paramSymss.flatten.filter(_.isTerm)
    val methodParamNames    = allTermParams.map(_.name).toSet
    val principalParamNames = allTermParams.collect {
      case sym if sym.tree match {
            case v: ValDef => v.tpt.tpe.dealias.typeSymbol.fullName == principalFullName
            case _         => false
          } =>
        sym.name
    }.toSet

    validateEndpoints(method, agentName, hasMount, methodParamNames, principalParamNames, headerVars)

    // --- Read-only extraction ---
    val readOnlyExpr = literal(extractReadOnly(method, principalParamNames.nonEmpty))

    '{
      MethodMetadata(
        name = ${
          Expr(methodName)
        },
        description = $descExpr,
        prompt = $promptExpr,
        mode = None,
        input = $inputSchema,
        output = $outputSchema,
        httpEndpoints = $endpointListExpr,
        readOnly = $readOnlyExpr
      )
    }
  }

  /**
   * Extract the optional `@readOnly(cache = ...)` annotation from a method and
   * build its metadata. `usesPrincipal` is derived from whether the method has
   * a Principal parameter (it is *not* user-configurable).
   */
  private def extractReadOnly(using
    Quotes
  )(method: quotes.reflect.Symbol, usesPrincipal: Boolean): Option[ReadOnlyConfig] = {
    import quotes.reflect.*

    val readOnlyAnnotations = method.annotations.collect {
      case ap @ Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.readOnly" =>
        ap -> args
    }

    if (readOnlyAnnotations.isEmpty) None
    else {
      val (_, args) = readOnlyAnnotations.head

      // Default cache policy if none specified.
      val cacheStr: String = args.collectFirst {
        case NamedArg("cache", Literal(StringConstant(v))) => v
        case Literal(StringConstant(v))                    => v
      }.getOrElse("until-write")

      val policy = CachePolicy.parse(cacheStr) match {
        case Left(err) =>
          report.errorAndAbort(s"@readOnly on method '${method.name}': $err")
        case Right(value) => value
      }

      Some(ReadOnlyConfig(policy, usesPrincipal))
    }
  }

  private def methodInputSchema(using Quotes)(method: quotes.reflect.Symbol): Expr[InputMetadata] = {
    import quotes.reflect.*

    val params = method.paramSymss.flatten.collect {
      case sym if sym.isTerm =>
        sym.tree match {
          case v: ValDef => (sym.name, v.tpt.tpe)
          case other     => report.errorAndAbort(s"Unsupported parameter declaration in ${method.name}: $other")
        }
    }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != "golem.Principal" }

    inputMetadataExpr(params)
  }

  /**
   * Build an `InputMetadata` from a list of user-supplied `(name, type)`
   * parameters: one `ParameterMetadata` per parameter carrying its
   * self-contained schema graph (the v2 `input-schema = parameters`).
   */
  private def inputMetadataExpr(using
    Quotes
  )(params: List[(String, quotes.reflect.TypeRepr)]): Expr[InputMetadata] = {
    val elements = params.map { case (name, tpe) =>
      val graphExpr = paramGraphExpr(tpe)
      '{ ParameterMetadata(${ Expr(name) }, FieldSource.UserSupplied, $graphExpr) }
    }
    '{ InputMetadata(${ Expr.ofList(elements) }) }
  }

  private def methodOutputSchema(using Quotes)(method: quotes.reflect.Symbol): Expr[OutputMetadata] = {
    import quotes.reflect.*

    method.tree match {
      case d: DefDef =>
        val outputType = unwrapAsyncType(d.returnTpt.tpe)
        outputMetadataExpr(outputType)
      case other =>
        report.errorAndAbort(s"Unable to read return type for ${method.name}: $other")
    }
  }

  /**
   * `Unit` output => `OutputMetadata.Unit` (the host returns `none`); any other
   * type => `OutputMetadata.Single` carrying its schema graph.
   */
  private def outputMetadataExpr(using Quotes)(tpe: quotes.reflect.TypeRepr): Expr[OutputMetadata] = {
    import quotes.reflect.*

    if (tpe =:= TypeRepr.of[Unit]) '{ OutputMetadata.Unit }
    else {
      val graphExpr = paramGraphExpr(tpe)
      '{ OutputMetadata.Single($graphExpr) }
    }
  }

  /** Summon `IntoSchema[t]` for `tpe` and produce its self-contained graph. */
  private def paramGraphExpr(using Quotes)(tpe: quotes.reflect.TypeRepr): Expr[SchemaGraph] = {
    import quotes.reflect.*

    tpe.asType match {
      case '[t] =>
        Expr.summon[IntoSchema[t]] match {
          case Some(into) => '{ $into.graph }
          case None       => report.errorAndAbort(s"No implicit IntoSchema available for type ${Type.show[t]}.$schemaHint")
        }
    }
  }

  private def detectAgentConfig(using
    Quotes
  )(traitRepr: quotes.reflect.TypeRepr): Option[Expr[List[AgentConfigDeclaration]]] = {
    import quotes.reflect.*

    val agentConfigBases = traitRepr.baseClasses.filter(_.fullName == "golem.config.AgentConfig")

    if (agentConfigBases.isEmpty) None
    else {
      val configTypes = agentConfigBases.flatMap { sym =>
        traitRepr.baseType(sym) match {
          case AppliedType(_, List(arg)) => Some(arg)
          case _                         => None
        }
      }

      if (configTypes.length > 1)
        report.errorAndAbort(s"Agent trait may extend at most one AgentConfig[T], found ${configTypes.length}")

      configTypes.headOption.map { configType =>
        configType.asType match {
          case '[t] =>
            Expr.summon[ConfigSchema[t]] match {
              case Some(schemaExpr) =>
                '{ $schemaExpr.describe(Nil) }
              case None =>
                report.errorAndAbort(
                  s"No implicit ConfigSchema available for config type ${Type.show[t]}.\n" +
                    "Hint: Add an implicit Schema[T] for your config type, which provides ConfigSchema automatically."
                )
            }
        }
      }
    }
  }

  private def inferIdSchema(using
    Quotes
  )(
    traitRepr: quotes.reflect.TypeRepr,
    descriptionExpr: Expr[String]
  ): Expr[ConstructorMetadata] = {
    import quotes.reflect.*

    val typeSymbol = traitRepr.typeSymbol
    val name       = typeSymbol.name

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
      case None if HttpDeclarationMacro.isRouter(typeSymbol) =>
        '{
          ConstructorMetadata(
            name = None,
            description = $descriptionExpr,
            promptHint = None,
            input = InputMetadata.empty
          )
        }
      case None =>
        report.errorAndAbort(
          s"Agent trait $name must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
        )
      case Some(classSym) =>
        val primaryCtor = classSym.primaryConstructor
        if (HttpDeclarationMacro.isRouter(typeSymbol) && primaryCtor.paramSymss.flatten.exists(_.isTerm))
          report.errorAndAbort("router-constructor: router constructors are parameterless")
        val params = primaryCtor.paramSymss.flatten.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef => (sym.name, v.tpt.tpe)
              case other     => report.errorAndAbort(s"Unsupported parameter declaration in Constructor: $other")
            }
        }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != "golem.Principal" }

        val inputMeta           = inputMetadataExpr(params)
        val ctorDescriptionExpr =
          docstringText(classSym) match {
            case Some(doc) => Expr(doc)
            case None      => descriptionExpr
          }
        '{ ConstructorMetadata(name = None, description = $ctorDescriptionExpr, promptHint = None, input = $inputMeta) }
    }
  }

  /**
   * Finds the Id constructor class and returns the names of any Principal-typed
   * parameters. Returns empty set if no Id class or no Principal params.
   */
  private def idConstructorPrincipalParams(using
    Quotes
  )(traitRepr: quotes.reflect.TypeRepr): Set[String] = {
    import quotes.reflect.*

    val typeSymbol   = traitRepr.typeSymbol
    val idFQN        = "golem.runtime.annotations.id"
    val principalFQN = "golem.Principal"

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
      case None           => Set.empty
      case Some(classSym) =>
        classSym.primaryConstructor.paramSymss.flatten.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef if v.tpt.tpe.dealias.typeSymbol.fullName == principalFQN => sym.name
              case _                                                                  => null
            }
        }.filter(_ != null).toSet
    }
  }

  /**
   * Cleaned Scaladoc text of a symbol, used as description fallback when no
   * `@description` annotation is present. Docstrings of symbols from other
   * compilation units are only visible when the compiler reads docs from TASTy
   * (`-Xread-docs` / `-Yread-docs`), which the Golem build plugins enable.
   */
  private def docstringText(using
    Quotes
  )(symbol: quotes.reflect.Symbol): Option[String] =
    symbol.docstring.flatMap(Scaladoc.clean)

  private def annotationString(using
    Quotes
  )(symbol: quotes.reflect.Symbol, annType: quotes.reflect.TypeRepr): Option[String] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), List(Literal(StringConstant(value)))) if tpt.tpe =:= annType =>
        value
    }
  }

  /**
   * Convert a `DurabilityMode` term from annotations into the wire-value string
   * without splicing the original term (which can carry invalid positions when
   * sourced from annotation trees under -Xcheck-macros).
   */
  private def durabilityWireExpr(using Quotes)(term: quotes.reflect.Term): Expr[String] = {
    import quotes.reflect.*
    def loop(t: Term): String =
      t match {
        case Inlined(_, _, inner: Term) => loop(inner)
        case _                          =>
          t.symbol.name match {
            // Scala may represent default annotation args via synthetic default-getter methods.
            // For `agentDefinition(mode: DurabilityMode = DurabilityMode.Durable)`, this is the default.
            case "$lessinit$greater$default$2" => "durable"
            case "Durable"                     => "durable"
            case "Ephemeral"                   => "ephemeral"
            case other                         =>
              report.errorAndAbort(
                s"Unsupported DurabilityMode annotation value: ${t.show} (symbol=$other). Use DurabilityMode.Durable or DurabilityMode.Ephemeral."
              )
          }
      }

    Expr(loop(term))
  }

  private def optionalString(using Quotes)(value: Option[String]): Expr[Option[String]] =
    value match {
      case Some(v) =>
        '{
          Some(${
            Expr(v)
          })
        }
      case None =>
        '{
          None
        }
    }

  private def optionalExprString(using Quotes)(value: Option[Expr[String]]): Expr[Option[String]] =
    value match {
      case Some(v) => '{ Some($v) }
      case None    => '{ None }
    }

  // ---------------------------------------------------------------------------
  // HTTP support helpers
  // ---------------------------------------------------------------------------

  /**
   * Extract a named String argument from the @agentDefinition annotation, with
   * positional fallback.
   */
  private def extractAgentDefinitionStringArg(using
    Quotes
  )(symbol: quotes.reflect.Symbol, argName: String, positionalIndex: Int): Option[String] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        args.collectFirst { case NamedArg(`argName`, Literal(StringConstant(value))) =>
          value
        }.orElse {
          if (positionalIndex >= 0) args.lift(positionalIndex).collect { case Literal(StringConstant(v)) => v }
          else None
        }
    }.flatten.map(_.trim).filter(_.nonEmpty)
  }

  /**
   * Extract a named Boolean argument from @agentDefinition, with positional
   * fallback.
   */
  private def extractAgentDefinitionBoolArg(using
    Quotes
  )(symbol: quotes.reflect.Symbol, argName: String, positionalIndex: Int): Option[Boolean] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        args.collectFirst { case NamedArg(`argName`, Literal(BooleanConstant(value))) =>
          value
        }.orElse {
          if (positionalIndex >= 0) args.lift(positionalIndex).collect { case Literal(BooleanConstant(v)) => v }
          else None
        }
    }.flatten
  }

  /**
   * Extract the cors Array[String] argument from @agentDefinition, with
   * positional fallback.
   */
  private def extractAgentDefinitionCorsArg(using
    Quotes
  )(symbol: quotes.reflect.Symbol, positionalIndex: Int): List[String] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
        args.collectFirst { case NamedArg("cors", arrayTerm) =>
          extractStringArray(arrayTerm)
        }.orElse {
          if (positionalIndex >= 0) args.lift(positionalIndex).map(extractStringArray)
          else None
        }.getOrElse(Nil)
    }.getOrElse(Nil)
  }

  /** Extract strings from an Array(...) literal tree. */
  private def extractStringArray(using Quotes)(term: quotes.reflect.Term): List[String] = {
    import quotes.reflect.*
    term match {
      case Inlined(_, _, inner) => extractStringArray(inner)
      case Typed(inner, _)      => extractStringArray(inner)
      // Curried form: Array.apply[T](elems*)(ClassTag[T]) — produced by Scala 3 for Array("a","b")
      case Apply(inner @ Apply(_, _), _)                => extractStringArray(inner)
      case Apply(_, List(Typed(Repeated(elems, _), _))) =>
        elems.collect { case Literal(StringConstant(s)) => s }
      case Apply(_, args) =>
        args.flatMap {
          case Typed(Repeated(elems, _), _) =>
            elems.collect { case Literal(StringConstant(s)) => s }
          case Literal(StringConstant(s)) => List(s)
          case _                          => Nil
        }
      case _ => Nil
    }
  }

  /**
   * Build the mount metadata from the @agentDefinition annotation.
   */
  private def extractHttpMount(using
    Quotes
  )(symbol: quotes.reflect.Symbol, agentName: String): Option[HttpMountDetails] = {
    import quotes.reflect.*

    val filesystem = HttpDeclarationMacro.mappingValues(symbol, "agentDefinition", "exposeFiles", 8)
    val mountPath  = extractAgentDefinitionStringArg(symbol, "mount", positionalIndex = 2)
    mountPath match {
      case None if filesystem.nonEmpty => report.errorAndAbort("exposeFiles requires an HTTP mount")
      case None                        => None
      case Some(mp)                    =>
        val pathSegments = HttpRouteParser.parsePathOnly(mp, "mount") match {
          case Left(err)                                                    => report.errorAndAbort(s"Invalid mount path in @agentDefinition for '$agentName': $err")
          case Right(segments) if HttpDeclarationMacro.exposesFiles(symbol) =>
            segments.map {
              case PathSegment.Literal(value) =>
                val decoded = golem.runtime.http.FileMappingParser
                  .publicPath("/" + value)
                  .fold(error => report.errorAndAbort(s"filesystem-mount: $error"), identity)
                PathSegment.Literal(decoded.head)
              case segment => segment
            }
          case Right(segments) => segments
        }

        val webhookSuffix = extractAgentDefinitionStringArg(symbol, "webhookSuffix", positionalIndex = 6) match {
          case None     => Nil
          case Some(ws) =>
            HttpRouteParser.parsePathOnly(ws, "webhookSuffix") match {
              case Left(err) =>
                report.errorAndAbort(s"Invalid webhookSuffix in @agentDefinition for '$agentName': $err")
              case Right(segments) => segments
            }
        }

        val authRequired = extractAgentDefinitionBoolArg(symbol, "auth", positionalIndex = 3).getOrElse(false)
        val phantomAgent = extractAgentDefinitionBoolArg(symbol, "phantomAgent", positionalIndex = 5).getOrElse(false)
        val corsPatterns = extractAgentDefinitionCorsArg(symbol, positionalIndex = 4)

        val mount = HttpMountDetails(
          pathPrefix = pathSegments,
          authRequired = authRequired,
          phantomAgent = phantomAgent,
          corsAllowedPatterns = corsPatterns,
          webhookSuffix = webhookSuffix,
          staticBindings = Nil,
          filesystemBindings = filesystem,
          openapiProviderMethod = None
        )
        HttpValidation.validateNoCatchAllInMount(agentName, mount) match {
          case Left(err) => report.errorAndAbort(err)
          case Right(()) => ()
        }

        Some(mount)
    }
  }

  private def literal[A: Type](value: A)(using Quotes): Expr[A] = {
    val core = new ToolMacroCore
    new CompiledWireMetadata[core.type](core).literal(value)
  }

  /** Extract @header annotations from method parameters. */
  private def extractHeaderVars(using Quotes)(method: quotes.reflect.Symbol): List[HeaderVariable] = {
    import quotes.reflect.*

    method.paramSymss.flatten.collect {
      case sym if sym.isTerm =>
        sym.annotations.collectFirst {
          case Apply(Select(New(tpt), _), List(Literal(StringConstant(headerName))))
              if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.header" =>
            HeaderVariable(headerName, sym.name)
          case Apply(Select(New(tpt), _), args)
              if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.header" =>
            val name = args.collectFirst {
              case Literal(StringConstant(n))                   => n
              case NamedArg("name", Literal(StringConstant(n))) => n
            }.getOrElse(
              report.errorAndAbort(
                s"@header annotation on parameter '${sym.name}' of method '${method.name}' must have a name argument"
              )
            )
            HeaderVariable(name, sym.name)
        }
    }.flatten
  }

  /**
   * Extract @endpoint annotations from a method and build metadata for each.
   */
  private def extractEndpoints(using
    Quotes
  )(method: quotes.reflect.Symbol, headerVars: List[HeaderVariable]): List[HttpEndpointDetails] = {
    import quotes.reflect.*

    def stringArg(args: List[Term], name: String, index: Int): Option[String] =
      args.collectFirst { case NamedArg(`name`, Literal(StringConstant(v))) => v }
        .orElse(args.lift(index).collect { case Literal(StringConstant(v)) => v })

    def stripTerm(term: Term): Term = term match {
      case Inlined(_, _, value) => stripTerm(value)
      case Typed(value, _)      => stripTerm(value)
      case NamedArg(_, value)   => stripTerm(value)
      case _                    => term
    }

    def isDefaultArg(term: Term): Boolean = {
      val value = stripTerm(term)
      value.symbol != Symbol.noSymbol && value.symbol.name.contains("$default$")
    }

    def durableStreamArg(args: List[Term], name: String, index: Int): Option[Term] =
      args.collectFirst { case NamedArg(`name`, value) if !isDefaultArg(value) => value }
        .orElse(args.lift(index).filter {
          case NamedArg(_, _) => false
          case value          => !isDefaultArg(value)
        })

    def durableStreamStringArg(
      args: List[Term],
      name: String,
      index: Int,
      annotation: String
    ): Option[String] =
      durableStreamArg(args, name, index).map { value =>
        stripTerm(value) match {
          case Literal(StringConstant(result)) => result
          case _                               => report.errorAndAbort(s"@$annotation argument '$name' must be a string literal", value.pos)
        }
      }

    def durableStreamBoolArg(args: List[Term], name: String, index: Int): Option[Boolean] =
      durableStreamArg(args, name, index).map { value =>
        stripTerm(value) match {
          case Literal(BooleanConstant(result)) => result
          case _                                => report.errorAndAbort(s"@durableStreams argument '$name' must be a boolean literal", value.pos)
        }
      }

    def durableStreamIntArg(args: List[Term], name: String, index: Int): Option[Int] =
      durableStreamArg(args, name, index).map { value =>
        stripTerm(value) match {
          case Literal(IntConstant(result)) => result
          case _                            => report.errorAndAbort(s"@durableStreams argument '$name' must be an integer literal", value.pos)
        }
      }

    val endpoints = method.annotations.collect {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.endpoint" =>
        EndpointSelector(stringArg(args, "method", 0).getOrElse(""), stringArg(args, "path", 1).getOrElse(""))
    }
    def selector(args: List[Term], annotation: String): EndpointSelector = {
      val requested = EndpointSelector(
        durableStreamStringArg(args, "endpointMethod", 0, annotation).getOrElse(""),
        durableStreamStringArg(args, "endpointPath", 1, annotation).getOrElse("")
      )
      if (requested.method.isEmpty != requested.path.isEmpty)
        report.errorAndAbort(
          s"@$annotation on method '${method.name}' must specify both endpointMethod and endpointPath"
        )
      val matches =
        if (requested.method.isEmpty) endpoints
        else endpoints.filter(e => e.method.equalsIgnoreCase(requested.method) && e.path == requested.path)
      if (matches.size != 1) {
        val reason =
          if (requested.method.isEmpty) "selector is ambiguous or missing"
          else "selector does not match exactly one @endpoint"
        report.errorAndAbort(s"@$annotation on method '${method.name}': $reason")
      }
      matches.head
    }
    val slots = method.annotations.collect {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.durableStreamSlot" =>
        val selected = selector(args, "durableStreamSlot")
        val source   = durableStreamStringArg(args, "source", 2, "durableStreamSlot").getOrElse(
          report.errorAndAbort(s"@durableStreamSlot on method '${method.name}' must specify source")
        )
        val slot = durableStreamStringArg(args, "slot", 3, "durableStreamSlot")
          .filter(_.nonEmpty)
          .getOrElse(report.errorAndAbort(s"@durableStreamSlot on method '${method.name}' must specify slot"))
        if (source != "input" && source != "output")
          report.errorAndAbort(s"@durableStreamSlot source must be 'input' or 'output', found '$source'")
        StreamSlot(
          selected,
          source,
          slot,
          durableStreamStringArg(args, "name", 4, "durableStreamSlot").filter(_.nonEmpty),
          durableStreamStringArg(args, "contentType", 5, "durableStreamSlot").filter(_.nonEmpty)
        )
    }.reverse
    slots
      .groupBy(s => (s.selector, s.source, s.slot))
      .collectFirst { case (key, values) if values.size > 1 => key }
      .foreach { key =>
        report.errorAndAbort(s"duplicate @durableStreamSlot for ${key._2} slot '${key._3}' on method '${method.name}'")
      }
    val routeOptions = method.annotations.collect {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.durableStreams" =>
        val selected = selector(args, "durableStreams")
        val readers  = durableStreamIntArg(args, "maxConcurrentReadersPerStream", 5)
        val appends  = durableStreamIntArg(args, "maxAppendRequestsPerSecondPerStream", 6)
        if (readers.exists(value => value <= 0 || value > 16))
          report.errorAndAbort("@durableStreams reader limit must be between 1 and 16")
        if (appends.exists(_ <= 0)) report.errorAndAbort("@durableStreams append limit must be positive")
        val writes = durableStreamBoolArg(args, "allowExternalWrites", 2)
        if (writes.contains(false) && appends.nonEmpty)
          report.errorAndAbort("@durableStreams append limit cannot be set when external writes are disabled")
        StreamOptions(
          selected,
          writes,
          durableStreamBoolArg(args, "allowStreamDelete", 3),
          durableStreamBoolArg(args, "allowInvocationDelete", 4),
          readers,
          appends
        )
    }
    routeOptions.groupBy(_.selector).collectFirst { case (key, values) if values.size > 1 => key }.foreach { key =>
      report.errorAndAbort(s"duplicate @durableStreams for ${key.method} ${key.path} on method '${method.name}'")
    }

    method.annotations.collect {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.endpoint" =>

        val methodStr = args.collectFirst {
          case Literal(StringConstant(v))                     => v
          case NamedArg("method", Literal(StringConstant(v))) => v
        }.getOrElse(
          report.errorAndAbort(s"@endpoint on method '${method.name}' must specify 'method'")
        )

        val pathStr = args.collectFirst { case NamedArg("path", Literal(StringConstant(v))) =>
          v
        }.orElse {
          // Second positional arg
          args.lift(1).collect { case Literal(StringConstant(v)) => v }
        }.getOrElse(
          report.errorAndAbort(s"@endpoint on method '${method.name}' must specify 'path'")
        )

        val authOverride: Option[Boolean] = args.collectFirst { case NamedArg("auth", Literal(BooleanConstant(v))) =>
          v
        }.orElse {
          args.lift(2).collect { case Literal(BooleanConstant(v)) => v }
        }

        val corsOverride: Option[List[String]] = args.collectFirst { case NamedArg("cors", arrayTerm) =>
          val strs = extractStringArray(arrayTerm)
          if (strs.isEmpty) None else Some(strs)
        }.orElse {
          args.lift(3).map { arrayTerm =>
            val strs = extractStringArray(arrayTerm)
            if (strs.isEmpty) None else Some(strs)
          }
        }.flatten

        // Parse the path at compile time
        val httpMethod = HttpMethod.fromString(methodStr) match {
          case Left(err) => report.errorAndAbort(s"@endpoint on method '${method.name}': $err")
          case Right(m)  => m
        }

        val parsed = HttpRouteParser.parse(pathStr) match {
          case Left(err) => report.errorAndAbort(s"@endpoint on method '${method.name}': $err")
          case Right(p)  => p
        }

        val selected       = EndpointSelector(methodStr, pathStr)
        val endpointSlots  = slots.filter(_.selector == selected)
        val options        = routeOptions.find(_.selector == selected)
        val durableStreams =
          if (endpointSlots.isEmpty && options.isEmpty) None
          else
            Some(
              DurableStreamRouteOptions(
                endpointSlots.map { slot =>
                  val source =
                    if (slot.source == "input") DurableStreamSlotSource.Input(slot.slot)
                    else DurableStreamSlotSource.Output(slot.slot)
                  DurableStreamSlotOptions(source, slot.name, slot.contentType)
                },
                options.flatMap(_.writes),
                options.flatMap(_.streamDelete),
                options.flatMap(_.invocationDelete),
                options.collect {
                  case option if option.readers.nonEmpty || option.appends.nonEmpty =>
                    DurableStreamRouteLoadOptions(option.readers, option.appends)
                }
              )
            )

        HttpEndpointDetails(
          httpMethod,
          parsed.pathSegments,
          headerVars,
          parsed.queryVars,
          authOverride,
          corsOverride,
          durableStreams
        )
    }
  }

  /**
   * Run compile-time validation of endpoint variables against method
   * parameters.
   */
  private def validateEndpoints(using
    Quotes
  )(
    method: quotes.reflect.Symbol,
    agentName: String,
    hasMount: Boolean,
    methodParamNames: Set[String],
    principalParamNames: Set[String],
    headerVars: List[HeaderVariable]
  ): Unit = {
    import quotes.reflect.*

    method.annotations.foreach {
      case Apply(Select(New(tpt), _), args)
          if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.endpoint" =>

        val methodStrOpt = args.collectFirst {
          case Literal(StringConstant(v))                     => v
          case NamedArg("method", Literal(StringConstant(v))) => v
        }

        val pathStrOpt = args.collectFirst { case NamedArg("path", Literal(StringConstant(v))) =>
          v
        }.orElse {
          args.lift(1).collect { case Literal(StringConstant(v)) => v }
        }

        for {
          methodStr  <- methodStrOpt
          pathStr    <- pathStrOpt
          httpMethod <- HttpMethod.fromString(methodStr).toOption
          parsed     <- HttpRouteParser.parse(pathStr).toOption
        } {
          val authOverride: Option[Boolean] = args.collectFirst { case NamedArg("auth", Literal(BooleanConstant(v))) =>
            v
          }

          val corsOverride: Option[List[String]] = args.collectFirst { case NamedArg("cors", arrayTerm) =>
            val strs = extractStringArray(arrayTerm)
            if (strs.isEmpty) None else Some(strs)
          }.flatten

          val endpoint = HttpEndpointDetails(
            httpMethod = httpMethod,
            pathSuffix = parsed.pathSegments,
            headerVars = headerVars,
            queryVars = parsed.queryVars,
            authOverride = authOverride,
            corsOverride = corsOverride
          )

          HttpValidation.validateEndpointVars(
            agentName,
            method.name,
            endpoint,
            methodParamNames,
            principalParamNames,
            hasMount
          ) match {
            case Left(err) => report.errorAndAbort(err)
            case Right(()) => ()
          }
        }

      case _ => ()
    }
  }

  private def unwrapAsyncType(using Quotes)(tpe: quotes.reflect.TypeRepr): quotes.reflect.TypeRepr = {
    import quotes.reflect.*
    tpe match {
      case AppliedType(constructor, args) if constructor.typeSymbol.fullName == "scala.concurrent.Future" =>
        args.headOption.getOrElse(TypeRepr.of[Unit])
      case AppliedType(constructor, args) if constructor.typeSymbol.fullName == "scala.scalajs.js.Promise" =>
        args.headOption.getOrElse(TypeRepr.of[Unit])
      case other =>
        other
    }
  }
}
