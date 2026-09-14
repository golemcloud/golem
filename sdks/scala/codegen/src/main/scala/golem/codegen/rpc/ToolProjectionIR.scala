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

package golem.codegen.rpc

import golem.codegen.discovery.SourceDiscovery

import scala.collection.mutable
import scala.meta._
import scala.meta.parsers._

/**
 * Shared semantic model for ordinary typed tool clients and middleware
 * projections. Renderers consume this model rather than independently deriving
 * command paths, canonical parameters, return shapes, and flattened leaves.
 */
object ToolProjectionIR {

  final case class Param(
    ident: String,
    typeExpr: String,
    projectedTypeExpr: String,
    kebab: String,
    aliases: List[String],
    scope: Option[String],
    kind: Option[String],
    isPrincipal: Boolean,
    isStdin: Boolean,
    isStdout: Boolean
  )

  final case class ProjectedParam(
    param: Param,
    canonicalName: String,
    canonicalAliases: List[String]
  )

  sealed trait ReturnShape
  final case class SubtreeReturn(childFqn: String) extends ReturnShape
  final case class LeafReturn(
    okType: Option[String],
    errType: Option[String],
    projectedOkType: Option[String],
    projectedErrType: Option[String],
    hasStdout: Boolean
  ) extends ReturnShape

  final case class Method(
    name: String,
    commandName: String,
    commandAliases: List[String],
    params: List[Param],
    returnTypeExpr: String,
    returnShape: ReturnShape,
    isRoot: Boolean,
    localCommandPath: List[String]
  )

  final case class FlattenedLeaf(
    name: String,
    ownerFqn: String,
    commandPath: List[String],
    params: List[ProjectedParam],
    underlyingParams: List[ProjectedParam],
    codec: LeafReturn
  )

  final case class Tool(
    path: String,
    pkg: String,
    name: String,
    toolName: String,
    projectionImports: List[String],
    ambientImportsRequired: Boolean,
    middlewareImportsRequired: Boolean,
    methods: List[Method],
    flattenedLeaves: List[FlattenedLeaf] = Nil
  ) {
    def fqn: String = if (pkg.isEmpty) name else s"$pkg.$name"
  }

  final case class Error(path: String, message: String)

  final case class Result(tools: List[Tool], errors: List[Error])

  private final case class UnresolvedMethod(
    name: String,
    nameOverride: Option[String],
    commandAliases: List[String],
    params: List[Param],
    returnTypeExpr: String
  )

  private final case class UnresolvedTool(
    path: String,
    pkg: String,
    name: String,
    toolName: String,
    imports: Map[String, String],
    wildcardImports: List[SourceDiscovery.WildcardImport],
    knownTypeFqns: Set[String],
    projectionImports: List[String],
    ambientImportsRequired: Boolean,
    middlewareImportsRequired: Boolean,
    methods: List[UnresolvedMethod]
  ) {
    def fqn: String = if (pkg.isEmpty) name else s"$pkg.$name"
  }

  def build(discovered: List[SourceDiscovery.ToolTrait]): Result = {
    val unresolved = discovered.map(toUnresolved)
    val initial    = unresolved.map { tool =>
      Tool(
        path = tool.path,
        pkg = tool.pkg,
        name = tool.name,
        toolName = tool.toolName,
        projectionImports = tool.projectionImports,
        ambientImportsRequired = tool.ambientImportsRequired,
        middlewareImportsRequired = tool.middlewareImportsRequired,
        methods = tool.methods.map { method =>
          val root        = kebabCase(method.name) == tool.toolName
          val commandName =
            if (root) tool.toolName else method.nameOverride.getOrElse(kebabCase(method.name))
          Method(
            name = method.name,
            commandName = commandName,
            commandAliases = method.commandAliases,
            params = method.params,
            returnTypeExpr = method.returnTypeExpr,
            returnShape = resolveReturnShape(tool, method, unresolved),
            isRoot = root,
            localCommandPath = if (root) Nil else List(commandName)
          )
        }
      )
    }

    val byFqn     = initial.map(tool => tool.fqn -> tool).toMap
    val flattened = initial.map { tool =>
      val result = flatten(tool, byFqn)
      tool.copy(flattenedLeaves = result.leaves) -> result.errors
    }
    val withLeaves = flattened.map(_._1)
    val errors     = flattened.flatMap(_._2) ++
      withLeaves.flatMap(tool => flattenedCollisionErrors(tool) ++ flattenedParameterErrors(tool))

    Result(withLeaves, errors.distinct)
  }

  private def toUnresolved(tool: SourceDiscovery.ToolTrait): UnresolvedTool = {
    val toolName = tool.toolName.getOrElse(kebabCase(tool.name))
    val methods  = tool.methods.map { method =>
      val params = method.params.map { param =>
        val kebab = kebabCase(param.name)
        val arg   = method.args.find(_.name == kebab)
        Param(
          ident = param.name,
          typeExpr = param.typeExpr,
          projectedTypeExpr = projectedTypeExpr(tool, param.typeExpr),
          kebab = kebab,
          aliases = arg.map(_.aliases).getOrElse(Nil),
          scope = arg.flatMap(_.scope),
          kind = arg.flatMap(_.kind),
          isPrincipal = resolvesTo(tool, param.typeExpr, "golem.Principal"),
          isStdin = resolvesTo(tool, param.typeExpr, "golem.tool.ToolInputStream"),
          isStdout = resolvesTo(tool, param.typeExpr, "golem.tool.ToolOutputStream")
        )
      }
      UnresolvedMethod(
        name = method.name,
        nameOverride = method.commandName,
        commandAliases = method.commandAliases,
        params = params,
        returnTypeExpr = method.returnTypeExpr
      )
    }
    UnresolvedTool(
      tool.path,
      tool.pkg,
      tool.name,
      toolName,
      tool.imports,
      tool.wildcardImports,
      tool.knownTypeFqns,
      projectionImports(tool),
      tool.methods.exists { method =>
        method.params
          .filterNot(param =>
            resolvesTo(tool, param.typeExpr, "golem.Principal") ||
              resolvesTo(tool, param.typeExpr, "golem.tool.ToolInputStream") ||
              resolvesTo(tool, param.typeExpr, "golem.tool.ToolOutputStream")
          )
          .exists(param => usesImportedType(typeContext(tool), param.typeExpr)) ||
        usesImportedType(typeContext(tool), method.returnTypeExpr)
      },
      tool.methods.exists { method =>
        method.params.exists(param => containsUnqualifiedType(projectedTypeExpr(tool, param.typeExpr))) ||
        containsUnqualifiedType(projectedTypeExpr(tool, method.returnTypeExpr))
      },
      methods
    )
  }

  private def resolveReturnShape(
    owner: UnresolvedTool,
    method: UnresolvedMethod,
    allTools: List[UnresolvedTool]
  ): ReturnShape =
    parseType(method.returnTypeExpr) match {
      case None =>
        LeafReturn(
          Some(method.returnTypeExpr),
          None,
          Some(projectedTypeExpr(owner, method.returnTypeExpr)),
          None,
          method.params.exists(_.isStdout)
        )
      case Some(tpe) =>
        resolveToolTrait(owner, tpe, allTools) match {
          case Some(child) => SubtreeReturn(child.fqn)
          case None        =>
            unwrapFuture(tpe) match {
              case Type.Apply.After_4_6_0(base, args) if lastNameOf(base).contains("Either") && args.values.size == 2 =>
                val err = args.values.head
                val ok  = args.values(1)
                LeafReturn(
                  okType = if (isUnitType(ok)) None else Some(ok.syntax),
                  errType = Some(err.syntax),
                  projectedOkType = if (isUnitType(ok)) None else Some(projectedType(owner, ok).syntax),
                  projectedErrType = Some(projectedType(owner, err).syntax),
                  hasStdout = method.params.exists(_.isStdout)
                )
              case other =>
                LeafReturn(
                  okType = if (isUnitType(other)) None else Some(other.syntax),
                  errType = None,
                  projectedOkType = if (isUnitType(other)) None else Some(projectedType(owner, other).syntax),
                  projectedErrType = None,
                  hasStdout = method.params.exists(_.isStdout)
                )
            }
        }
    }

  private final case class TypeContext(
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[SourceDiscovery.WildcardImport],
    knownTypeFqns: Set[String]
  )

  private val scalaDefaultTypes =
    Set(
      "Any",
      "AnyRef",
      "AnyVal",
      "Array",
      "BigDecimal",
      "BigInt",
      "Boolean",
      "Byte",
      "Char",
      "Double",
      "Either",
      "Float",
      "Int",
      "Left",
      "List",
      "Long",
      "Map",
      "Nothing",
      "Null",
      "Option",
      "Right",
      "Seq",
      "Set",
      "Short",
      "Some",
      "Unit",
      "Vector"
    ).map(name => name -> s"scala.$name").toMap ++
      Map(
        "Map"    -> "scala.collection.immutable.Map",
        "Set"    -> "scala.collection.immutable.Set",
        "String" -> "java.lang.String"
      )

  private val knownExternalTypes = Set(
    "golem.Principal",
    "golem.tool.ToolInputStream",
    "golem.tool.ToolOutputStream"
  )

  private def typeContext(tool: SourceDiscovery.ToolTrait): TypeContext =
    TypeContext(tool.pkg, tool.imports, tool.wildcardImports, tool.knownTypeFqns)

  private def typeContext(tool: UnresolvedTool): TypeContext =
    TypeContext(tool.pkg, tool.imports, tool.wildcardImports, tool.knownTypeFqns)

  private def normalizeFqn(value: String): String =
    value.stripPrefix("_root_.")

  private def enclosingPackages(pkg: String): List[String] = {
    val parts = pkg.split('.').toList.filter(_.nonEmpty)
    parts.indices.reverse.map(index => parts.take(index + 1).mkString(".")).toList
  }

  private def importedCandidates(context: TypeContext, ref: String): List[String] = {
    val rooted     = ref.startsWith("_root_.")
    val normalized = normalizeFqn(ref)
    val relative   =
      if (!rooted && normalized.contains("."))
        enclosingPackages(context.pkg).map(prefix => s"$prefix.$normalized")
      else Nil
    if (rooted) List(normalized)
    else (relative :+ normalized).distinct
  }

  private def knownImportedType(context: TypeContext, ref: String): Option[String] =
    importedCandidates(context, ref).find(candidate =>
      context.knownTypeFqns.contains(candidate) || knownExternalTypes.contains(candidate)
    )

  private def projectionImports(tool: SourceDiscovery.ToolTrait): List[String] = {
    val named = tool.imports.toList.sortBy(_._1).map { case (alias, imported) =>
      val normalized = imported.stripPrefix("_root_.")
      val dot        = normalized.lastIndexOf('.')
      val original   = if (dot < 0) normalized else normalized.substring(dot + 1)
      val owner      = if (dot < 0) "" else normalized.substring(0, dot)
      val prefix     = if (imported.startsWith("_root_.")) "_root_." else ""
      if (alias == original) s"import $prefix$normalized"
      else s"import $prefix$owner.{$original => $alias}"
    }
    val wildcard = tool.wildcardImports.map { imported =>
      val exclusions = imported.excludes.toList.sorted
      if (exclusions.isEmpty) s"import ${imported.pkg}._"
      else s"import ${imported.pkg}.{${exclusions.map(name => s"$name => _").mkString(", ")}, _}"
    }
    (named ++ wildcard).distinct
  }

  private def resolveSimpleType(context: TypeContext, name: String): String = {
    val explicit = context.imports.get(name).flatMap(knownImportedType(context, _))
    val local    = {
      val candidate = if (context.pkg.isEmpty) name else s"${context.pkg}.$name"
      if (context.knownTypeFqns.contains(candidate)) Some(candidate) else None
    }
    val wildcardCandidates = context.wildcardImports.reverse.flatMap { wildcard =>
      if (wildcard.excludes.contains(name)) None
      else knownImportedType(context, s"${wildcard.pkg}.$name")
    }.distinct

    explicit
      .orElse(local)
      .orElse(wildcardCandidates.headOption)
      .orElse(scalaDefaultTypes.get(name))
      .getOrElse(name)
  }

  private def resolveSelectedType(context: TypeContext, syntax: String): String = {
    val normalized = normalizeFqn(syntax)
    val dot        = normalized.indexOf('.')
    if (dot < 0) resolveSimpleType(context, normalized)
    else {
      val first = normalized.substring(0, dot)
      val rest  = normalized.substring(dot + 1)
      context.imports
        .get(first)
        .flatMap(imported => knownImportedType(context, s"$imported.$rest"))
        .getOrElse {
          knownImportedType(context, syntax).getOrElse(normalized)
        }
    }
  }

  private def resolvedTypeFqn(context: TypeContext, tpe: Type): Option[String] =
    tpe match {
      case Type.Name(name)     => Some(resolveSimpleType(context, name))
      case select: Type.Select => Some(resolveSelectedType(context, select.syntax))
      case _                   => None
    }

  private def projectedTypeExpr(context: TypeContext, expr: String): String =
    parseType(expr) match {
      case None       => expr
      case Some(root) =>
        def atomicRefs(tree: Tree): List[(Int, Int, String)] =
          tree match {
            case name: Type.Name =>
              val resolved = resolveSimpleType(context, name.value)
              val rendered = if (resolved.contains('.')) s"_root_.$resolved" else resolved
              List((name.pos.start, name.pos.end, rendered))
            case select: Type.Select =>
              val resolved = resolveSelectedType(context, select.syntax)
              val rendered =
                if (select.syntax.startsWith("_root_.") || knownImportedType(context, resolved).contains(resolved))
                  s"_root_.$resolved"
                else resolved
              List((select.pos.start, select.pos.end, rendered))
            case other =>
              other.children.toList.flatMap(atomicRefs)
          }

        atomicRefs(root).sortBy { case (start, _, _) => -start }
          .foldLeft(expr) { case (current, (start, end, replacement)) =>
            current.substring(0, start) + replacement + current.substring(end)
          }
    }

  private def usesImportedType(context: TypeContext, expr: String): Boolean =
    parseType(expr).exists {
      def loop(tree: Tree): Boolean =
        tree match {
          case Type.Apply.After_4_6_0(base, args)
              if lastNameOf(base).exists(name => name == "Future" || name == "Either") =>
            args.values.exists(loop)
          case name: Type.Name =>
            val local =
              context.knownTypeFqns.contains(
                if (context.pkg.isEmpty) name.value else s"${context.pkg}.${name.value}"
              )
            !scalaDefaultTypes.contains(name.value) &&
            !local &&
            (context.imports.contains(name.value) ||
              context.wildcardImports.exists(imported => !imported.excludes.contains(name.value)))
          case select: Type.Select =>
            val syntax = select.syntax.stripPrefix("_root_.")
            val first  = syntax.takeWhile(_ != '.')
            !select.syntax.startsWith("_root_.") && context.imports.contains(first)
          case other => other.children.exists(loop)
        }
      loop(_)
    }

  private def containsUnqualifiedType(expr: String): Boolean =
    parseType(expr).exists {
      def loop(tree: Tree): Boolean =
        tree match {
          case _: Type.Name        => true
          case select: Type.Select => !select.syntax.startsWith("_root_.")
          case other               => other.children.exists(loop)
        }
      loop(_)
    }

  private def projectedType(context: TypeContext, tpe: Type): Type =
    parseType(projectedTypeExpr(context, tpe.syntax)).getOrElse(tpe)

  private def projectedType(tool: UnresolvedTool, tpe: Type): Type =
    projectedType(typeContext(tool), tpe)

  private def projectedTypeExpr(tool: SourceDiscovery.ToolTrait, expr: String): String =
    projectedTypeExpr(typeContext(tool), expr)

  private def projectedTypeExpr(tool: UnresolvedTool, expr: String): String =
    projectedTypeExpr(typeContext(tool), expr)

  private def resolvesTo(tool: SourceDiscovery.ToolTrait, expr: String, expectedFqn: String): Boolean =
    parseType(expr)
      .flatMap(resolvedTypeFqn(typeContext(tool), _))
      .contains(expectedFqn)

  private def resolveToolTrait(
    owner: UnresolvedTool,
    tpe: Type,
    allTools: List[UnresolvedTool]
  ): Option[UnresolvedTool] =
    resolvedTypeFqn(typeContext(owner), tpe)
      .flatMap(fqn => allTools.find(_.fqn == fqn))
      .orElse {
        tpe match {
          case Type.Name(name) if !owner.imports.contains(name) =>
            allTools.filter(_.name == name) match {
              case single :: Nil => Some(single)
              case _             => None
            }
          case _ => None
        }
      }

  private final case class FlattenResult(leaves: List[FlattenedLeaf], errors: List[Error])

  private def flatten(root: Tool, byFqn: Map[String, Tool]): FlattenResult = {
    val leaves = List.newBuilder[FlattenedLeaf]
    val errors = List.newBuilder[Error]

    def visit(
      tool: Tool,
      commandPrefix: List[String],
      inheritedParams: List[ProjectedParam],
      omitted: List[String],
      visited: List[String]
    ): Unit =
      tool.methods.foreach { method =>
        method.returnShape match {
          case SubtreeReturn(childFqn) if !visited.contains(childFqn) =>
            byFqn.get(childFqn).foreach { child =>
              val navigationParams = keptSubtreeParams(tool, method, omitted).map(projected(tool, method, _))
              val childOmitted     = childOmittedSurfaces(tool, method, omitted)
              visit(
                child,
                commandPrefix ++ method.localCommandPath,
                inheritedParams ++ navigationParams,
                childOmitted,
                visited :+ childFqn
              )
            }
          case SubtreeReturn(childFqn) =>
            val cycleStart  = visited.indexOf(childFqn)
            val traitCycle  = (visited.drop(cycleStart) :+ childFqn).mkString(" -> ")
            val commandPath = commandPrefix ++ method.localCommandPath
            errors += Error(
              root.path,
              s"Cannot generate ${root.name}Middleware: subtree cycle at command path `${commandPath.mkString(" ")}` ($traitCycle)."
            )
          case leaf: LeafReturn =>
            val local  = keptMiddlewareLeafParams(tool, method, omitted).map(projected(tool, method, _))
            val params = inheritedParams ++ local
            val path   = commandPrefix ++ method.localCommandPath
            leaves += FlattenedLeaf(
              name = method.name,
              ownerFqn = tool.fqn,
              commandPath = path,
              params = params,
              underlyingParams = params.filterNot(_.param.isPrincipal),
              codec = leaf
            )
        }
      }

    visit(root, Nil, Nil, Nil, List(root.fqn))
    FlattenResult(leaves.result(), errors.result())
  }

  private def flattenedCollisionErrors(tool: Tool): List[Error] =
    tool.flattenedLeaves
      .groupBy(_.name)
      .toList
      .sortBy(_._1)
      .flatMap { case (name, leaves) =>
        if (leaves.size < 2) Nil
        else {
          val paths = leaves
            .map(leaf => if (leaf.commandPath.isEmpty) "<root>" else leaf.commandPath.mkString(" "))
            .distinct
            .sorted
          List(
            Error(
              tool.path,
              s"Cannot generate ${tool.name}Middleware: flattened method `$name` conflicts between command paths ${paths.mkString("`", "`, `", "`")}. Rename one Scala method."
            )
          )
        }
      }

  private def flattenedParameterErrors(tool: Tool): List[Error] =
    tool.flattenedLeaves.flatMap { leaf =>
      val duplicateNames = leaf.params
        .groupBy(_.param.ident)
        .collect {
          case (name, occurrences) if occurrences.size > 1 => name
        }
        .toList
        .sorted
      val reservedNames = leaf.params
        .map(_.param.ident)
        .filter(name => name == "underlying" || name.startsWith("__golem"))
        .distinct
        .sorted
      val path = if (leaf.commandPath.isEmpty) "<root>" else leaf.commandPath.mkString(" ")

      duplicateNames.map { name =>
        Error(
          tool.path,
          s"Cannot generate ${tool.name}Middleware.${leaf.name} for command path `$path`: flattened parameter name `$name` occurs more than once. Rename one Scala parameter."
        )
      } ++ reservedNames.map { name =>
        Error(
          tool.path,
          s"Cannot generate ${tool.name}Middleware.${leaf.name} for command path `$path`: parameter name `$name` is reserved by middleware projections."
        )
      }
    }

  private[codegen] def kebabCase(ident: String): String = {
    val out             = new StringBuilder
    val chars           = ident.toCharArray
    var i               = 0
    def pushSep(): Unit =
      if (out.nonEmpty && out.last != '-') out += '-'
    while (i < chars.length) {
      val c = chars(i)
      if (c == '_' || c == '-') pushSep()
      else if (c.isUpper) {
        val prev     = if (i > 0) Some(chars(i - 1)) else None
        val next     = if (i + 1 < chars.length) Some(chars(i + 1)) else None
        val boundary =
          prev.exists(p => p.isLower || p.isDigit) ||
            (prev.exists(_.isUpper) && next.exists(_.isLower))
        if (boundary) pushSep()
        out += c.toLower
      } else out += c
      i += 1
    }
    out.result()
  }

  private[codegen] def parseType(expr: String): Option[Type] =
    dialects.Scala3(expr).parse[Type].toOption

  private[codegen] def lastNameOf(tpe: Type): Option[String] =
    tpe match {
      case Type.Name(n)                    => Some(n)
      case Type.Select(_, Type.Name(n))    => Some(n)
      case Type.Apply.After_4_6_0(base, _) => lastNameOf(base)
      case _                               => None
    }

  private[codegen] def lastTypeName(expr: String): Option[String] =
    parseType(expr).flatMap(lastNameOf)

  private[codegen] def unwrapFuture(tpe: Type): Type =
    tpe match {
      case Type.Apply.After_4_6_0(base, args) if lastNameOf(base).contains("Future") && args.values.size == 1 =>
        args.values.head
      case _ => tpe
    }

  private[codegen] def isUnitType(tpe: Type): Boolean =
    lastNameOf(tpe).contains("Unit") || tpe.syntax == "Unit"

  private[codegen] def rootMethodOf(tool: Tool): Option[Method] =
    tool.methods.find(_.isRoot)

  private[codegen] def reachableTools(root: Tool, toolsByFqn: Map[String, Tool]): List[Tool] = {
    val result  = List.newBuilder[Tool]
    val visited = mutable.Set.empty[String]

    def visit(tool: Tool): Unit =
      if (visited.add(tool.fqn)) {
        result += tool
        tool.methods.foreach {
          case Method(_, _, _, _, _, SubtreeReturn(childFqn), _, _) =>
            toolsByFqn.get(childFqn).foreach(visit)
          case _ => ()
        }
      }

    visit(root)
    result.result()
  }

  private[codegen] def paramSurfaces(param: Param): List[String] =
    (param.kebab :: param.aliases).distinct

  private def surfacesIntersect(
    leftName: String,
    leftAliases: List[String],
    rightName: String,
    rightAliases: List[String]
  ): Boolean =
    leftName == rightName ||
      leftAliases.contains(rightName) ||
      rightAliases.contains(leftName) ||
      leftAliases.exists(rightAliases.contains)

  private[codegen] def isGlobalParam(param: Param): Boolean =
    param.scope.contains("global")

  private[codegen] def isFlagParam(param: Param): Boolean =
    param.kind.exists(kind => kind == "flag" || kind == "count-flag") ||
      lastTypeName(param.typeExpr).contains("Boolean")

  private[codegen] def isCountFlag(param: Param): Boolean =
    param.kind.contains("count-flag")

  private[codegen] def isStreamParam(param: Param): Boolean =
    param.isStdin || param.isStdout

  private[codegen] def inheritedRootParams(tool: Tool, method: Method): List[Param] =
    if (method.isRoot) Nil
    else
      rootMethodOf(tool) match {
        case None       => Nil
        case Some(root) =>
          root.params.filter(isGlobalParam).filterNot { rootParam =>
            method.params.exists { own =>
              surfacesIntersect(rootParam.kebab, rootParam.aliases, own.kebab, own.aliases)
            }
          }
      }

  private[codegen] def canonicalValueName(tool: Tool, method: Method, param: Param): String =
    if (method.isRoot) param.kebab
    else
      rootMethodOf(tool).flatMap { root =>
        root.params.find { rootParam =>
          isGlobalParam(rootParam) &&
          surfacesIntersect(rootParam.kebab, rootParam.aliases, param.kebab, param.aliases)
        }
      }
        .map(_.kebab)
        .getOrElse(param.kebab)

  private[codegen] def canonicalAliases(tool: Tool, method: Method, param: Param): List[String] =
    rootMethodOf(tool).flatMap { root =>
      root.params.find { rootParam =>
        isGlobalParam(rootParam) &&
        surfacesIntersect(rootParam.kebab, rootParam.aliases, param.kebab, param.aliases)
      }
    }
      .map(_.aliases)
      .getOrElse(param.aliases)

  private[codegen] def omittedMatches(tool: Tool, method: Method, param: Param, omitted: List[String]): Boolean =
    paramSurfaces(param).exists(omitted.contains) || {
      !method.isRoot &&
      rootMethodOf(tool).exists { root =>
        root.params.exists { rootParam =>
          isGlobalParam(rootParam) &&
          paramSurfaces(rootParam).exists(omitted.contains) &&
          surfacesIntersect(rootParam.kebab, rootParam.aliases, param.kebab, param.aliases)
        }
      }
    }

  private[codegen] def childOmittedSurfaces(
    tool: Tool,
    method: Method,
    inheritedOmitted: List[String]
  ): List[String] = {
    val out = mutable.LinkedHashSet.empty[String]
    inheritedOmitted.foreach(out.add)
    inheritedRootParams(tool, method)
      .filterNot(param => omittedMatches(tool, method, param, inheritedOmitted))
      .foreach(param => paramSurfaces(param).foreach(out.add))
    method.params
      .filterNot(param =>
        param.isPrincipal || isStreamParam(param) || omittedMatches(tool, method, param, inheritedOmitted)
      )
      .foreach { param =>
        out.add(canonicalValueName(tool, method, param))
        canonicalAliases(tool, method, param).foreach(out.add)
      }
    out.toList
  }

  private[codegen] def keptLeafParams(tool: Tool, method: Method, omitted: List[String]): List[Param] =
    (inheritedRootParams(tool, method) ++ method.params).filter { param =>
      !param.isPrincipal && !param.isStdout && !omittedMatches(tool, method, param, omitted)
    }

  private def keptMiddlewareLeafParams(tool: Tool, method: Method, omitted: List[String]): List[Param] =
    (inheritedRootParams(tool, method) ++ method.params).filter { param =>
      !param.isStdout && !omittedMatches(tool, method, param, omitted)
    }

  private[codegen] def keptSubtreeParams(tool: Tool, method: Method, omitted: List[String]): List[Param] =
    (inheritedRootParams(tool, method) ++ method.params).filter { param =>
      !param.isPrincipal && !omittedMatches(tool, method, param, omitted)
    }

  private[codegen] def projected(tool: Tool, method: Method, param: Param): ProjectedParam =
    ProjectedParam(
      param,
      canonicalValueName(tool, method, param),
      canonicalAliases(tool, method, param)
    )
}
