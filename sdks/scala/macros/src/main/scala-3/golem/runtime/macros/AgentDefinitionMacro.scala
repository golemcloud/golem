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

import golem.config.{AgentConfigDeclaration, ConfigSchema}
import golem.data.{ElementSchema, GolemSchema, NamedElementSchema, StructuredSchema}
import golem.runtime.annotations.{description, prompt}
import golem.runtime.{AgentMetadata, MethodMetadata, Snapshotting, SnapshottingConfig}
import golem.runtime.http.{
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
  private val schemaHint: String =
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  inline def generate[T]: AgentMetadata = ${ impl[T] }

  private def impl[T: Type](using Quotes): Expr[AgentMetadata] = {
    import quotes.reflect.*

    val typeRepr   = TypeRepr.of[T]
    val typeSymbol = typeRepr.typeSymbol

    if !typeSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"@agent target must be a trait, found: ${typeSymbol.fullName}")

    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name

    val hasAgentDefinition =
      typeSymbol.annotations.exists {
        case Apply(Select(New(tpt), _), _)
            if tpt.tpe.dealias.typeSymbol.fullName == "golem.runtime.annotations.agentDefinition" =>
          true
        case _ => false
      }

    val agentTypeName =
      agentDefinitionTypeName(typeSymbol).map(validateTypeName).getOrElse {
        if !hasAgentDefinition then
          report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${typeSymbol.fullName}")
        defaultTypeNameFromTrait(typeSymbol)
      }

    val traitDescription = annotationString(typeSymbol, TypeRepr.of[description])
    // Note: `@agentDefinition` has a default `mode = Durable`. We omit that default in metadata via
    // `agentDefinitionMode`.
    val traitMode = agentDefinitionMode(typeSymbol)

    // --- HTTP mount extraction from @agentDefinition ---
    val httpMountExpr: Expr[Option[HttpMountDetails]] = extractHttpMount(typeSymbol, agentTypeName)
    val hasMount                                      = extractAgentDefinitionStringArg(typeSymbol, "mount", positionalIndex = 2).exists(_.nonEmpty)

    val methods = typeSymbol.methodMembers.collect {
      case method if method.flags.is(Flags.Deferred) && method.isDefDef =>
        methodMetadata(method, agentTypeName, hasMount)
    }

    val idSchema = inferIdSchema(typeRepr)

    // --- Mount-level Principal validation ---
    if (hasMount) {
      val mountPath         = extractAgentDefinitionStringArg(typeSymbol, "mount", positionalIndex = 2).getOrElse("")
      val mountSegments     = HttpRouteParser.parsePathOnly(mountPath, "mount").getOrElse(Nil)
      val idPrincipalParams = idConstructorPrincipalParams(typeRepr)
      if (idPrincipalParams.nonEmpty) {
        val mount = HttpMountDetails(mountSegments, false, false, Nil, Nil)
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

    '{
      AgentMetadata(
        name = ${
          Expr(agentTypeName)
        },
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
    val descExpr     = optionalString(annotationString(method, TypeRepr.of[description]))
    val promptExpr   = optionalString(annotationString(method, TypeRepr.of[prompt]))
    val inputSchema  = methodInputSchema(method)
    val outputSchema = methodOutputSchema(method)

    // --- HTTP endpoint extraction ---
    val headerVars       = extractHeaderVars(method)
    val endpointDetails  = extractEndpoints(method, headerVars)
    val endpointListExpr = Expr.ofList(endpointDetails)

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
        httpEndpoints = $endpointListExpr
      )
    }
  }

  private def methodInputSchema(using Quotes)(method: quotes.reflect.Symbol): Expr[StructuredSchema] = {
    import quotes.reflect.*

    val params = method.paramSymss.flatten.collect {
      case sym if sym.isTerm =>
        sym.tree match {
          case v: ValDef => (sym.name, v.tpt.tpe)
          case other     => report.errorAndAbort(s"Unsupported parameter declaration in ${method.name}: $other")
        }
    }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != "golem.Principal" }

    if params.isEmpty then
      '{
        StructuredSchema.Tuple(Nil)
      }
    else {
      val elements = params.map { case (name, tpe) =>
        val elementExpr = elementSchemaExpr(name, tpe)
        '{
          NamedElementSchema(
            ${
              Expr(name)
            },
            $elementExpr
          )
        }
      }
      val listExpr = Expr.ofList(elements)
      '{
        StructuredSchema.Tuple($listExpr)
      }
    }
  }

  private def methodOutputSchema(using Quotes)(method: quotes.reflect.Symbol): Expr[StructuredSchema] = {
    import quotes.reflect.*

    method.tree match {
      case d: DefDef =>
        val outputType = unwrapAsyncType(d.returnTpt.tpe)
        structuredSchemaExpr(outputType)
      case other =>
        report.errorAndAbort(s"Unable to read return type for ${method.name}: $other")
    }
  }

  private def structuredSchemaExpr(using Quotes)(tpe: quotes.reflect.TypeRepr): Expr[StructuredSchema] = {
    import quotes.reflect.*

    tpe.asType match {
      case '[t] =>
        Expr.summon[GolemSchema[t]] match {
          case Some(codecExpr) => '{ $codecExpr.schema }
          case None            => report.errorAndAbort(s"No implicit GolemSchema available for type ${Type.show[t]}.$schemaHint")
        }
    }
  }

  private def elementSchemaExpr(using
    Quotes
  )(@scala.annotation.unused paramName: String, tpe: quotes.reflect.TypeRepr): Expr[golem.data.ElementSchema] = {
    import quotes.reflect.*

    tpe.asType match {
      case '[t] =>
        Expr.summon[GolemSchema[t]] match {
          case Some(codecExpr) =>
            '{ $codecExpr.elementSchema }
          case None =>
            report.errorAndAbort(s"No implicit GolemSchema available for type ${Type.show[t]}.$schemaHint")
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
    traitRepr: quotes.reflect.TypeRepr
  ): Expr[StructuredSchema] = {
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
      case None =>
        report.errorAndAbort(
          s"Agent trait $name must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
        )
      case Some(classSym) =>
        val primaryCtor = classSym.primaryConstructor
        val params      = primaryCtor.paramSymss.flatten.collect {
          case sym if sym.isTerm =>
            sym.tree match {
              case v: ValDef => (sym.name, v.tpt.tpe)
              case other     => report.errorAndAbort(s"Unsupported parameter declaration in Constructor: $other")
            }
        }

        if params.isEmpty then '{ StructuredSchema.Tuple(Nil) }
        else {
          val elements = params.map { case (name, tpe) =>
            val elementExpr = elementSchemaExpr(name, tpe)
            '{
              NamedElementSchema(
                ${ Expr(name) },
                $elementExpr
              )
            }
          }
          val listExpr = Expr.ofList(elements)
          '{ StructuredSchema.Tuple($listExpr) }
        }
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
      case Inlined(_, _, inner)                         => extractStringArray(inner)
      case Typed(inner, _)                              => extractStringArray(inner)
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
   * Build an Expr[Option[HttpMountDetails]] from the @agentDefinition
   * annotation.
   */
  private def extractHttpMount(using
    Quotes
  )(symbol: quotes.reflect.Symbol, agentName: String): Expr[Option[HttpMountDetails]] = {
    import quotes.reflect.*

    val mountPath = extractAgentDefinitionStringArg(symbol, "mount", positionalIndex = 2)
    mountPath match {
      case None     => '{ None }
      case Some(mp) =>
        val pathSegments = HttpRouteParser.parsePathOnly(mp, "mount") match {
          case Left(err)       => report.errorAndAbort(s"Invalid mount path in @agentDefinition for '$agentName': $err")
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

        val pathExpr    = pathSegmentsExpr(pathSegments)
        val webhookExpr = pathSegmentsExpr(webhookSuffix)
        val authExpr    = Expr(authRequired)
        val phantomExpr = Expr(phantomAgent)
        val corsExpr    = Expr.ofList(corsPatterns.map(Expr(_)))

        val mount = HttpMountDetails(
          pathPrefix = pathSegments,
          authRequired = authRequired,
          phantomAgent = phantomAgent,
          corsAllowedPatterns = corsPatterns,
          webhookSuffix = webhookSuffix
        )
        HttpValidation.validateNoCatchAllInMount(agentName, mount) match {
          case Left(err) => report.errorAndAbort(err)
          case Right(()) => ()
        }

        '{
          Some(
            HttpMountDetails(
              pathPrefix = $pathExpr,
              authRequired = $authExpr,
              phantomAgent = $phantomExpr,
              corsAllowedPatterns = $corsExpr,
              webhookSuffix = $webhookExpr
            )
          )
        }
    }
  }

  /**
   * Convert a compile-time List[PathSegment] into an Expr[List[PathSegment]].
   */
  private def pathSegmentsExpr(using Quotes)(segments: List[PathSegment]): Expr[List[PathSegment]] = {
    val exprs = segments.map {
      case PathSegment.Literal(v)               => '{ PathSegment.Literal(${ Expr(v) }) }
      case PathSegment.PathVariable(v)          => '{ PathSegment.PathVariable(${ Expr(v) }) }
      case PathSegment.RemainingPathVariable(v) => '{ PathSegment.RemainingPathVariable(${ Expr(v) }) }
      case PathSegment.SystemVariable(v)        => '{ PathSegment.SystemVariable(${ Expr(v) }) }
    }
    Expr.ofList(exprs)
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
   * Extract @endpoint annotations from a method and build
   * Expr[HttpEndpointDetails] for each.
   */
  private def extractEndpoints(using
    Quotes
  )(method: quotes.reflect.Symbol, headerVars: List[HeaderVariable]): List[Expr[HttpEndpointDetails]] = {
    import quotes.reflect.*

    val headerVarsExpr = Expr.ofList(headerVars.map { hv =>
      '{ HeaderVariable(${ Expr(hv.headerName) }, ${ Expr(hv.variableName) }) }
    })

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

        val pathExpr  = pathSegmentsExpr(parsed.pathSegments)
        val queryExpr = Expr.ofList(parsed.queryVars.map { qv =>
          '{ QueryVariable(${ Expr(qv.queryParamName) }, ${ Expr(qv.variableName) }) }
        })
        val httpMethodExpr = httpMethod match {
          case HttpMethod.Get       => '{ HttpMethod.Get }
          case HttpMethod.Post      => '{ HttpMethod.Post }
          case HttpMethod.Put       => '{ HttpMethod.Put }
          case HttpMethod.Delete    => '{ HttpMethod.Delete }
          case HttpMethod.Patch     => '{ HttpMethod.Patch }
          case HttpMethod.Head      => '{ HttpMethod.Head }
          case HttpMethod.Options   => '{ HttpMethod.Options }
          case HttpMethod.Connect   => '{ HttpMethod.Connect }
          case HttpMethod.Trace     => '{ HttpMethod.Trace }
          case HttpMethod.Custom(m) => '{ HttpMethod.Custom(${ Expr(m) }) }
        }
        val authExpr = authOverride match {
          case None    => '{ None: Option[Boolean] }
          case Some(v) => '{ Some(${ Expr(v) }): Option[Boolean] }
        }
        val corsExpr = corsOverride match {
          case None    => '{ None: Option[List[String]] }
          case Some(v) => '{ Some(${ Expr.ofList(v.map(Expr(_))) }): Option[List[String]] }
        }

        '{
          HttpEndpointDetails(
            httpMethod = $httpMethodExpr,
            pathSuffix = $pathExpr,
            headerVars = $headerVarsExpr,
            queryVars = $queryExpr,
            authOverride = $authExpr,
            corsOverride = $corsExpr
          )
        }
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
