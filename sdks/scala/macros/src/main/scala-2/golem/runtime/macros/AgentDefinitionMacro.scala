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

import golem.config.ConfigSchema
import golem.data.GolemSchema
import golem.runtime.{AgentMetadata, Snapshotting, SnapshottingConfig}
import golem.runtime.http.{
  HeaderVariable,
  HttpEndpointDetails,
  HttpMethod,
  HttpMountDetails,
  HttpRouteParser,
  HttpValidation,
  PathSegment
}

import scala.reflect.macros.blackbox

object AgentDefinitionMacro {
  def generate[T]: AgentMetadata = macro AgentDefinitionMacroImpl.impl[T]
}

object AgentDefinitionMacroImpl {
  private val schemaHint: String =
    "\nHint: GolemSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "Scala 3: `final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n" +
      "Scala 2: `implicit val schema: zio.blocks.schema.Schema[T] = zio.blocks.schema.Schema.derived`.\n"
  def impl[T: c.WeakTypeTag](c: blackbox.Context): c.Expr[AgentMetadata] = {
    import c.universe._

    val tpe        = weakTypeOf[T]
    val typeSymbol = tpe.typeSymbol

    if (!typeSymbol.isClass || !typeSymbol.asClass.isTrait) {
      c.abort(c.enclosingPosition, s"@agent target must be a trait, found: ${typeSymbol.fullName}")
    }

    val agentDefinitionFQN                             = "golem.runtime.annotations.agentDefinition"
    def isAgentDefinitionAnn(ann: Annotation): Boolean =
      ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN
    def defaultTypeNameFromTrait(sym: Symbol): String =
      sym.name.decodedName.toString

    val rawTypeName: String =
      typeSymbol.annotations.collectFirst {
        case ann if isAgentDefinitionAnn(ann) =>
          ann.tree.children.tail.collectFirst { case Literal(Constant(s: String)) => s }.getOrElse("")
      }
        .getOrElse("")

    val agentTypeName: String = {
      val trimmed  = rawTypeName.trim
      val resolved =
        if (trimmed.nonEmpty) trimmed
        else {
          val hasAnn = typeSymbol.annotations.exists(a => isAgentDefinitionAnn(a))
          if (!hasAnn)
            c.abort(c.enclosingPosition, s"Missing @agentDefinition(...) on agent trait: ${typeSymbol.fullName}")
          defaultTypeNameFromTrait(typeSymbol)
        }
      validateTypeName(resolved)
    }

    val descriptionType = typeOf[golem.runtime.annotations.description]
    val promptType      = typeOf[golem.runtime.annotations.prompt]
    val endpointType    = typeOf[golem.runtime.annotations.endpoint]
    val headerType      = typeOf[golem.runtime.annotations.header]

    val traitDescription = annotationString(c)(typeSymbol, descriptionType)
    val traitMode        =
      agentDefinitionModeWireValueExpr(c)(typeSymbol, agentDefinitionFQN)

    val httpMountOpt = extractHttpMount(c)(typeSymbol, agentTypeName, agentDefinitionFQN)
    val hasMount     = httpMountOpt.isDefined

    val methods = tpe.decls.collect {
      case method: MethodSymbol if method.isAbstract && method.isMethod && method.name.toString != "new" =>
        methodMetadata(c)(method, descriptionType, promptType, endpointType, headerType, agentTypeName, hasMount)
    }.toList

    val idSchema = inferIdSchema(c)(tpe)

    // --- Mount-level Principal validation ---
    if (hasMount) {
      val idPrincipalParams = idConstructorPrincipalParams(c)(tpe)
      if (idPrincipalParams.nonEmpty) {
        val annOpt = typeSymbol.annotations.find(ann =>
          ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN
        )
        val mountStr = annOpt.flatMap { ann =>
          extractNamedStringArg(c)(ann.tree.children.tail, "mount", 2)
        }.getOrElse("")
        if (mountStr.nonEmpty) {
          val mountSegments = HttpRouteParser.parsePathOnly(mountStr, "mount").getOrElse(Nil)
          val mount         = HttpMountDetails(mountSegments, false, false, Nil, Nil)
          HttpValidation.validateMountVarsAreNotPrincipal(agentTypeName, mount, idPrincipalParams) match {
            case Left(err) => c.abort(c.enclosingPosition, err)
            case Right(()) => ()
          }
        }
      }
    }

    val typeName      = agentTypeName
    val traitDescExpr = optionalStringExpr(c)(traitDescription)
    val traitModeExpr = optionalTreeExpr(c)(traitMode)
    val httpMountExpr = httpMountOpt match {
      case Some(tree) => q"_root_.scala.Some($tree)"
      case None       => q"_root_.scala.None"
    }

    val configExpr = detectAgentConfig(c)(tpe) match {
      case Some(tree) => tree
      case None       => q"_root_.scala.Nil"
    }

    val snapshottingExpr = extractSnapshottingExpr(c)(typeSymbol, agentDefinitionFQN)

    c.Expr[AgentMetadata](q"""
      _root_.golem.runtime.AgentMetadata(
        name = $typeName,
        description = $traitDescExpr,
        mode = $traitModeExpr,
        methods = List(..$methods),
        constructor = $idSchema,
        httpMount = $httpMountExpr,
        config = $configExpr,
        snapshotting = $snapshottingExpr
      )
    """)
  }

  private def validateTypeName(value: String): String =
    value

  private def methodMetadata(c: blackbox.Context)(
    method: c.universe.MethodSymbol,
    descriptionType: c.universe.Type,
    promptType: c.universe.Type,
    endpointType: c.universe.Type,
    headerType: c.universe.Type,
    agentName: String,
    hasMount: Boolean
  ): c.Tree = {
    import c.universe._

    val methodName   = method.name.toString
    val descExpr     = optionalStringExpr(c)(annotationString(c)(method, descriptionType))
    val promptExpr   = optionalStringExpr(c)(annotationString(c)(method, promptType))
    val inputSchema  = methodInputSchema(c)(method)
    val outputSchema = methodOutputSchema(c)(method)

    val principalFullName = "golem.Principal"
    val allParams         = method.paramLists.flatten.filter(_.isTerm)
    val paramNames        = allParams.collect {
      case p if p.typeSignature.dealias.typeSymbol.fullName != principalFullName => p.name.toString
    }.toSet
    val principalParamNames = allParams.collect {
      case p if p.typeSignature.dealias.typeSymbol.fullName == principalFullName => p.name.toString
    }.toSet
    val headerVarMap  = extractHeaderAnnotations(c)(method, headerType)
    val endpointTrees = extractEndpoints(c)(
      method,
      endpointType,
      headerVarMap,
      agentName,
      methodName,
      paramNames,
      principalParamNames,
      hasMount
    )

    q"""
      _root_.golem.runtime.MethodMetadata(
        name = $methodName,
        description = $descExpr,
        prompt = $promptExpr,
        mode = _root_.scala.None,
        input = $inputSchema,
        output = $outputSchema,
        httpEndpoints = _root_.scala.List(..$endpointTrees)
      )
    """
  }

  private def methodInputSchema(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree = {
    import c.universe._

    val principalFullName = "golem.Principal"
    val params            = method.paramLists.flatten.collect {
      case param if param.isTerm => (param.name.toString, param.typeSignature)
    }.filter { case (_, tpe) => tpe.dealias.typeSymbol.fullName != principalFullName }

    if (params.isEmpty) {
      q"_root_.golem.data.StructuredSchema.Tuple(Nil)"
    } else if (params.length == 1) {
      val (_, paramType) = params.head
      structuredSchemaExpr(c)(paramType)
    } else {
      val elements = params.map { case (name, tpe) =>
        val schemaExpr = elementSchemaExpr(c)(name, tpe)
        q"_root_.golem.data.NamedElementSchema($name, $schemaExpr)"
      }
      q"_root_.golem.data.StructuredSchema.Tuple(List(..$elements))"
    }
  }

  private def methodOutputSchema(c: blackbox.Context)(method: c.universe.MethodSymbol): c.Tree = {
    val outputType = unwrapAsyncType(c)(method.returnType)
    structuredSchemaExpr(c)(outputType)
  }

  private def structuredSchemaExpr(c: blackbox.Context)(tpe: c.universe.Type): c.Tree = {
    import c.universe._

    val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, tpe)
    val schemaInstance  = c.inferImplicitValue(golemSchemaType)

    if (schemaInstance.isEmpty) {
      c.abort(c.enclosingPosition, s"No implicit GolemSchema available for type $tpe.$schemaHint")
    }

    q"$schemaInstance.schema"
  }

  private def elementSchemaExpr(
    c: blackbox.Context
  )(@annotation.unused paramName: String, tpe: c.universe.Type): c.Tree = {
    import c.universe._

    val golemSchemaType = appliedType(typeOf[GolemSchema[_]].typeConstructor, tpe)
    val schemaInstance  = c.inferImplicitValue(golemSchemaType)

    if (schemaInstance.isEmpty) {
      c.abort(c.enclosingPosition, s"No implicit GolemSchema available for type $tpe.$schemaHint")
    }

    q"$schemaInstance.elementSchema"
  }

  private def inferIdSchema(c: blackbox.Context)(tpe: c.universe.Type): c.Tree = {
    import c.universe._

    val idAnnotationType = typeOf[golem.runtime.annotations.id]

    val annotatedClass = tpe.members.collectFirst {
      case sym
          if sym.isClass && !sym.isMethod &&
            sym.annotations.exists(ann => ann.tree.tpe != null && ann.tree.tpe =:= idAnnotationType) =>
        sym
    }

    val constructorClass = annotatedClass.orElse {
      val byName = tpe.member(TypeName("Id"))
      if (byName == NoSymbol) None else Some(byName)
    }.getOrElse {
      val name = tpe.typeSymbol.name.decodedName.toString
      c.abort(
        c.enclosingPosition,
        s"Agent trait $name must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
      )
    }

    val primaryCtor = constructorClass.asClass.primaryConstructor.asMethod
    val params      = primaryCtor.paramLists.flatten.filter(_.isTerm).map(p => (p.name.toString, p.typeSignature))

    if (params.isEmpty) q"_root_.golem.data.StructuredSchema.Tuple(Nil)"
    else {
      val elements = params.map { case (name, paramTpe) =>
        val schemaExpr = elementSchemaExpr(c)(name, paramTpe)
        q"_root_.golem.data.NamedElementSchema($name, $schemaExpr)"
      }
      q"_root_.golem.data.StructuredSchema.Tuple(List(..$elements))"
    }
  }

  /**
   * Finds the Id constructor class and returns the names of any Principal-typed
   * parameters. Returns empty set if no Id class or no Principal params.
   */
  private def idConstructorPrincipalParams(c: blackbox.Context)(tpe: c.universe.Type): Set[String] = {
    import c.universe._

    val idAnnotationType  = typeOf[golem.runtime.annotations.id]
    val principalFullName = "golem.Principal"

    val annotatedClass = tpe.members.collectFirst {
      case sym
          if sym.isClass && !sym.isMethod &&
            sym.annotations.exists(ann => ann.tree.tpe != null && ann.tree.tpe =:= idAnnotationType) =>
        sym
    }

    val constructorClass = annotatedClass.orElse {
      val byName = tpe.member(TypeName("Id"))
      if (byName == NoSymbol) None else Some(byName)
    }

    constructorClass match {
      case None           => Set.empty
      case Some(classSym) =>
        val primaryCtor = classSym.asClass.primaryConstructor.asMethod
        primaryCtor.paramLists.flatten
          .filter(_.isTerm)
          .collect {
            case p if p.typeSignature.dealias.typeSymbol.fullName == principalFullName => p.name.toString
          }
          .toSet
    }
  }

  private def detectAgentConfig(c: blackbox.Context)(tpe: c.universe.Type): Option[c.Tree] = {
    import c.universe._

    val agentConfigBases = tpe.baseClasses.filter(_.fullName == "golem.config.AgentConfig")

    if (agentConfigBases.isEmpty) None
    else {
      val configTypes = agentConfigBases.flatMap { sym =>
        tpe.baseType(sym).typeArgs.headOption
      }

      if (configTypes.length > 1)
        c.abort(c.enclosingPosition, s"Agent trait may extend at most one AgentConfig[T], found ${configTypes.length}")

      configTypes.headOption.map { configType =>
        val configSchemaType     = appliedType(typeOf[ConfigSchema[_]].typeConstructor, configType)
        val configSchemaInstance = c.inferImplicitValue(configSchemaType)

        if (configSchemaInstance.isEmpty) {
          c.abort(
            c.enclosingPosition,
            s"No implicit ConfigSchema available for config type $configType.\n" +
              "Hint: Add an implicit Schema[T] for your config type, which provides ConfigSchema automatically."
          )
        }

        q"$configSchemaInstance.describe(_root_.scala.Nil)"
      }
    }
  }

  private def extractSnapshottingExpr(c: blackbox.Context)(
    typeSymbol: c.universe.Symbol,
    agentDefinitionFQN: String
  ): c.Tree = {
    import c.universe._

    val annOpt =
      typeSymbol.annotations.find(ann => ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN)
    val snapStr = annOpt.flatMap { ann =>
      val args = ann.tree.children.tail
      extractNamedStringArg(c)(args, "snapshotting", 7)
    }.getOrElse("disabled")

    Snapshotting.parse(snapStr) match {
      case Right(Snapshotting.Disabled) =>
        q"_root_.golem.runtime.Snapshotting.Disabled"
      case Right(Snapshotting.Enabled(SnapshottingConfig.Default)) =>
        q"_root_.golem.runtime.Snapshotting.Enabled(_root_.golem.runtime.SnapshottingConfig.Default)"
      case Right(Snapshotting.Enabled(SnapshottingConfig.Periodic(nanos))) =>
        val nanosLit = Literal(Constant(nanos))
        q"_root_.golem.runtime.Snapshotting.Enabled(_root_.golem.runtime.SnapshottingConfig.Periodic($nanosLit))"
      case Right(Snapshotting.Enabled(SnapshottingConfig.EveryN(count))) =>
        val countLit = Literal(Constant(count))
        q"_root_.golem.runtime.Snapshotting.Enabled(_root_.golem.runtime.SnapshottingConfig.EveryN($countLit))"
      case Left(err) =>
        c.abort(c.enclosingPosition, s"Invalid snapshotting on @agentDefinition for ${typeSymbol.fullName}: $err")
    }
  }

  private def extractHttpMount(c: blackbox.Context)(
    typeSymbol: c.universe.Symbol,
    agentName: String,
    agentDefinitionFQN: String
  ): Option[c.Tree] = {
    import c.universe._

    val annOpt =
      typeSymbol.annotations.find(ann => ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN)
    annOpt.flatMap { ann =>
      val args         = ann.tree.children.tail
      val mountStr     = extractNamedStringArg(c)(args, "mount", 2).getOrElse("")
      val authRequired = extractNamedBooleanArg(c)(args, "auth", 3).getOrElse(false)
      val corsPatterns = extractNamedStringArrayArg(c)(args, "cors", 4).getOrElse(Nil)
      val phantomAgent = extractNamedBooleanArg(c)(args, "phantomAgent", 5).getOrElse(false)
      val webhookStr   = extractNamedStringArg(c)(args, "webhookSuffix", 6).getOrElse("")

      if (mountStr.isEmpty) None
      else {
        val pathPrefix = HttpRouteParser.parsePathOnly(mountStr, "mount path") match {
          case Right(segments) => segments
          case Left(err)       => c.abort(c.enclosingPosition, s"Invalid HTTP mount path on agent '$agentName': $err")
        }
        val webhookSuffix =
          if (webhookStr.isEmpty) Nil
          else
            HttpRouteParser.parsePathOnly(webhookStr, "webhookSuffix") match {
              case Right(segments) => segments
              case Left(err)       => c.abort(c.enclosingPosition, s"Invalid webhookSuffix on agent '$agentName': $err")
            }

        val mount = HttpMountDetails(pathPrefix, authRequired, phantomAgent, corsPatterns, webhookSuffix)
        HttpValidation.validateNoCatchAllInMount(agentName, mount) match {
          case Left(err) => c.abort(c.enclosingPosition, err)
          case Right(()) => ()
        }

        val prefixTrees  = pathPrefix.map(seg => pathSegmentTree(c)(seg))
        val corsTrees    = corsPatterns.map(p => q"$p")
        val webhookTrees = webhookSuffix.map(seg => pathSegmentTree(c)(seg))

        Some(q"""
          _root_.golem.runtime.http.HttpMountDetails(
            pathPrefix = _root_.scala.List(..$prefixTrees),
            authRequired = $authRequired,
            phantomAgent = $phantomAgent,
            corsAllowedPatterns = _root_.scala.List(..$corsTrees),
            webhookSuffix = _root_.scala.List(..$webhookTrees)
          )
        """)
      }
    }
  }

  private def pathSegmentTree(c: blackbox.Context)(seg: PathSegment): c.Tree = {
    import c.universe._
    seg match {
      case PathSegment.Literal(value)              => q"_root_.golem.runtime.http.PathSegment.Literal($value)"
      case PathSegment.PathVariable(name)          => q"_root_.golem.runtime.http.PathSegment.PathVariable($name)"
      case PathSegment.RemainingPathVariable(name) =>
        q"_root_.golem.runtime.http.PathSegment.RemainingPathVariable($name)"
      case PathSegment.SystemVariable(name) => q"_root_.golem.runtime.http.PathSegment.SystemVariable($name)"
    }
  }

  private def extractHeaderAnnotations(c: blackbox.Context)(
    method: c.universe.MethodSymbol,
    headerType: c.universe.Type
  ): Map[String, String] = {
    import c.universe._
    method.paramLists.flatten.collect {
      case param if param.isTerm =>
        val paramName = param.name.toString
        param.annotations.collectFirst {
          case ann if ann.tree.tpe != null && ann.tree.tpe =:= headerType =>
            ann.tree.children.tail.collectFirst { case Literal(Constant(headerName: String)) => headerName }
        }.flatten.map(headerName => paramName -> headerName)
    }.flatten.toMap
  }

  private def extractEndpoints(c: blackbox.Context)(
    method: c.universe.MethodSymbol,
    endpointType: c.universe.Type,
    headerVarMap: Map[String, String],
    agentName: String,
    methodName: String,
    paramNames: Set[String],
    principalParamNames: Set[String],
    hasMount: Boolean
  ): List[c.Tree] = {
    import c.universe._

    method.annotations.filter(ann => ann.tree.tpe != null && ann.tree.tpe =:= endpointType).map { ann =>
      val args = ann.tree.children.tail

      val httpMethodStr = extractNamedStringArg(c)(args, "method", 0)
        .getOrElse(c.abort(c.enclosingPosition, s"@endpoint on method '$methodName' requires a 'method' argument"))
      val pathStr = extractNamedStringArg(c)(args, "path", 1)
        .getOrElse(c.abort(c.enclosingPosition, s"@endpoint on method '$methodName' requires a 'path' argument"))
      val authOverrideOpt = extractNamedBooleanArg(c)(args, "auth", 2)
      val corsPatterns    = extractNamedStringArrayArg(c)(args, "cors", 3).getOrElse(Nil)

      val httpMethod = HttpMethod.fromString(httpMethodStr) match {
        case Right(m)  => m
        case Left(err) => c.abort(c.enclosingPosition, s"@endpoint on method '$methodName': $err")
      }

      val parsed = HttpRouteParser.parse(pathStr) match {
        case Right(p)  => p
        case Left(err) =>
          c.abort(c.enclosingPosition, s"@endpoint on method '$methodName': invalid path '$pathStr': $err")
      }

      val headerVars = headerVarMap.map { case (varName, headerName) =>
        HeaderVariable(headerName, varName)
      }.toList

      val authOverride: Option[Boolean]      = authOverrideOpt
      val corsOverride: Option[List[String]] = if (corsPatterns.isEmpty) None else Some(corsPatterns)

      val endpointDetails =
        HttpEndpointDetails(httpMethod, parsed.pathSegments, headerVars, parsed.queryVars, authOverride, corsOverride)
      HttpValidation.validateEndpointVars(
        agentName,
        methodName,
        endpointDetails,
        paramNames,
        principalParamNames,
        hasMount
      ) match {
        case Left(err) => c.abort(c.enclosingPosition, err)
        case Right(()) => ()
      }

      val methodTree  = httpMethodTree(c)(httpMethod)
      val pathTrees   = parsed.pathSegments.map(seg => pathSegmentTree(c)(seg))
      val headerTrees = headerVars.map { hv =>
        q"_root_.golem.runtime.http.HeaderVariable(${hv.headerName}, ${hv.variableName})"
      }
      val queryTrees = parsed.queryVars.map { qv =>
        q"_root_.golem.runtime.http.QueryVariable(${qv.queryParamName}, ${qv.variableName})"
      }
      val authOverrideTree = authOverride match {
        case Some(v) => q"_root_.scala.Some($v)"
        case None    => q"_root_.scala.None"
      }
      val corsOverrideTree = corsOverride match {
        case Some(patterns) =>
          val ts = patterns.map(p => q"$p")
          q"_root_.scala.Some(_root_.scala.List(..$ts))"
        case None => q"_root_.scala.None"
      }

      q"""
        _root_.golem.runtime.http.HttpEndpointDetails(
          httpMethod = $methodTree,
          pathSuffix = _root_.scala.List(..$pathTrees),
          headerVars = _root_.scala.List(..$headerTrees),
          queryVars = _root_.scala.List(..$queryTrees),
          authOverride = $authOverrideTree,
          corsOverride = $corsOverrideTree
        )
      """
    }
  }

  private def httpMethodTree(c: blackbox.Context)(method: HttpMethod): c.Tree = {
    import c.universe._
    method match {
      case HttpMethod.Get       => q"_root_.golem.runtime.http.HttpMethod.Get"
      case HttpMethod.Post      => q"_root_.golem.runtime.http.HttpMethod.Post"
      case HttpMethod.Put       => q"_root_.golem.runtime.http.HttpMethod.Put"
      case HttpMethod.Delete    => q"_root_.golem.runtime.http.HttpMethod.Delete"
      case HttpMethod.Patch     => q"_root_.golem.runtime.http.HttpMethod.Patch"
      case HttpMethod.Head      => q"_root_.golem.runtime.http.HttpMethod.Head"
      case HttpMethod.Options   => q"_root_.golem.runtime.http.HttpMethod.Options"
      case HttpMethod.Connect   => q"_root_.golem.runtime.http.HttpMethod.Connect"
      case HttpMethod.Trace     => q"_root_.golem.runtime.http.HttpMethod.Trace"
      case HttpMethod.Custom(m) => q"_root_.golem.runtime.http.HttpMethod.Custom($m)"
    }
  }

  private def extractNamedStringArg(c: blackbox.Context)(
    args: List[c.universe.Tree],
    name: String,
    positionalIndex: Int
  ): Option[String] = {
    import c.universe._
    args.collectFirst { case NamedArg(Ident(TermName(`name`)), Literal(Constant(v: String))) =>
      v
    }.orElse {
      args.lift(positionalIndex).collect {
        case Literal(Constant(v: String))              => v
        case NamedArg(_, Literal(Constant(v: String))) => v
      }
    }.filter(_.nonEmpty)
  }

  private def extractNamedBooleanArg(c: blackbox.Context)(
    args: List[c.universe.Tree],
    name: String,
    positionalIndex: Int
  ): Option[Boolean] = {
    import c.universe._
    args.collectFirst { case NamedArg(Ident(TermName(`name`)), Literal(Constant(v: Boolean))) =>
      v
    }.orElse {
      args.lift(positionalIndex).collect {
        case Literal(Constant(v: Boolean))              => v
        case NamedArg(_, Literal(Constant(v: Boolean))) => v
      }
    }
  }

  private def extractNamedStringArrayArg(c: blackbox.Context)(
    args: List[c.universe.Tree],
    name: String,
    positionalIndex: Int
  ): Option[List[String]] = {
    import c.universe._
    def extractArray(tree: Tree): Option[List[String]] = tree match {
      case Apply(_, elems) =>
        val strings = elems.collect { case Literal(Constant(s: String)) => s }
        if (strings.length == elems.length) Some(strings) else None
      case _ => None
    }
    def unwrapNamedArg(tree: Tree): Tree = tree match {
      case NamedArg(_, v) => v
      case other          => other
    }
    args.collectFirst { case NamedArg(Ident(TermName(`name`)), arr) =>
      extractArray(arr)
    }.flatten.orElse {
      args.lift(positionalIndex).flatMap(t => extractArray(unwrapNamedArg(t)))
    }
  }

  private def annotationString(
    c: blackbox.Context
  )(symbol: c.universe.Symbol, annType: c.universe.Type): Option[String] = {
    import c.universe._

    symbol.annotations.collectFirst {
      case ann if ann.tree.tpe =:= annType =>
        ann.tree.children.tail.collectFirst { case Literal(Constant(value: String)) =>
          value
        }
    }.flatten
  }

  private def agentDefinitionModeWireValueExpr(
    c: blackbox.Context
  )(symbol: c.universe.Symbol, annFQN: String): Option[c.Tree] = {
    import c.universe._
    symbol.annotations.collectFirst {
      case ann if ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == annFQN =>
        // agentDefinition(typeName: String = "", mode: DurabilityMode = null)
        ann.tree.children.tail.drop(1).headOption.map {
          case Literal(Constant(value: String)) =>
            // (Legacy) allow stringly-typed values.
            val v = value.trim.toLowerCase
            if (v == "durable") EmptyTree else Literal(Constant(v))
          case Literal(Constant(null)) =>
            EmptyTree
          case Select(_, TermName("Durable")) =>
            // Treat default Durable as unset (omit defaults in metadata)
            EmptyTree
          case Ident(TermName("Durable")) =>
            EmptyTree
          case other =>
            q"$other.wireValue()"
        }
    }.flatten.filter(_ != EmptyTree)
  }

  private def optionalStringExpr(c: blackbox.Context)(value: Option[String]): c.Tree = {
    import c.universe._
    value match {
      case Some(v) => q"Some($v)"
      case None    => q"None"
    }
  }

  private def optionalTreeExpr(c: blackbox.Context)(value: Option[c.Tree]): c.Tree = {
    import c.universe._
    value match {
      case Some(v) => q"Some($v)"
      case None    => q"None"
    }
  }

  private def unwrapAsyncType(c: blackbox.Context)(tpe: c.universe.Type): c.universe.Type = {
    import c.universe._

    val futureSymbol = typeOf[scala.concurrent.Future[_]].typeSymbol

    tpe match {
      case TypeRef(_, sym, args) if sym == futureSymbol && args.nonEmpty =>
        args.head
      case TypeRef(_, sym, args) if sym.fullName == "scala.scalajs.js.Promise" && args.nonEmpty =>
        args.head
      case _ =>
        tpe
    }
  }
}
