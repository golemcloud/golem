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
 * Code generator for typed tool RPC clients.
 *
 * For every discovered `@toolDefinition` trait `T` this writes `TClient.scala`
 * containing:
 *   - `trait TClient` with one method per tool command (the agent-author call
 *     surface: leaf commands return `Future[Either[ToolError[E], Result]]`,
 *     subtree commands return child client values carrying the inherited
 *     canonical-input prefix)
 *   - `object TClient` with `toolName`, `apply()`, the lazily cached tool
 *     descriptor / per-command canonical input models / error schemas, and the
 *     SDK-internal client and subtree-wrapper classes
 *
 * The generated code mirrors the Rust SDK's client macro: `Principal`
 * parameters are excluded, stdout parameters move into the result tuple, stdin
 * stays a parameter, root-level global arguments are inherited by subcommand
 * signatures, and subtree navigation packs inherited globals into the
 * canonical-input prefix of the child call.
 */
object ToolRpcCodegen {

  final case class GeneratedFile(relativePath: String, content: String)

  final case class Warning(message: String)

  final case class Result(
    files: Seq[GeneratedFile],
    warnings: Seq[Warning]
  )

  // ── Internal surface model ─────────────────────────────────────────────────

  private final case class Param(
    ident: String,
    typeExpr: String,
    kebab: String,
    aliases: List[String],
    scope: Option[String],
    kind: Option[String],
    isPrincipal: Boolean,
    isStdin: Boolean,
    isStdout: Boolean
  )

  private final case class Method(
    name: String,
    nameOverride: Option[String],
    params: List[Param],
    returnTypeExpr: String
  )

  private final case class Tool(
    pkg: String,
    name: String,
    toolName: String,
    methods: List[Method]
  ) {
    def fqn: String = if (pkg.isEmpty) name else s"$pkg.$name"
  }

  private sealed trait ReturnShape
  private final case class SubtreeReturn(child: Tool)                                  extends ReturnShape
  private final case class LeafReturn(okType: Option[String], errType: Option[String]) extends ReturnShape

  def generate(
    tools: List[SourceDiscovery.ToolTrait],
    existingObjects: Seq[SourceDiscovery.ExistingObject]
  ): Result = {
    val warnings = List.newBuilder[Warning]
    val files    = List.newBuilder[GeneratedFile]

    val surfaces = tools.map(toSurface)

    val existingByFqn: Set[String] = existingObjects.map { obj =>
      if (obj.pkg.isEmpty) obj.name else s"${obj.pkg}.${obj.name}"
    }.toSet

    surfaces.foreach { tool =>
      val clientName = s"${tool.name}Client"
      val clientFqn  = if (tool.pkg.isEmpty) clientName else s"${tool.pkg}.$clientName"

      if (existingByFqn.contains(clientFqn)) {
        warnings += Warning(
          s"Skipping tool RPC client generation for ${tool.fqn}: " +
            s"object $clientFqn already exists. Remove the handwritten client to enable codegen."
        )
      } else {
        val generator   = new FileGenerator(tool, surfaces, warnings)
        val content     = generator.generate()
        val packagePath =
          if (tool.pkg.isEmpty) ""
          else tool.pkg.replace('.', '/') + "/"
        files += GeneratedFile(s"$packagePath$clientName.scala", content)
      }
    }

    Result(files = files.result(), warnings = warnings.result())
  }

  private def toSurface(t: SourceDiscovery.ToolTrait): Tool = {
    val toolName = t.toolName.getOrElse(kebabCase(t.name))
    val methods  = t.methods.map { m =>
      val params = m.params.map { p =>
        val kebab = kebabCase(p.name)
        val arg   = m.args.find(_.name == kebab)
        val last  = lastTypeName(p.typeExpr)
        Param(
          ident = p.name,
          typeExpr = p.typeExpr,
          kebab = kebab,
          aliases = arg.map(_.aliases).getOrElse(Nil),
          scope = arg.flatMap(_.scope),
          kind = arg.flatMap(_.kind),
          isPrincipal = last.contains("Principal"),
          isStdin = last.contains("ToolInputStream"),
          isStdout = last.contains("ToolOutputStream")
        )
      }
      Method(m.name, m.commandName, params, m.returnTypeExpr)
    }
    Tool(t.pkg, t.name, toolName, methods)
  }

  // ── Name conversion (port of the Rust SDK's to_kebab_case) ────────────────

  private[rpc] def kebabCase(ident: String): String = {
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

  private def pascalCase(input: String): String = {
    val out        = new StringBuilder
    var capitalize = true
    input.foreach { ch =>
      if (ch == '_' || ch == '-') capitalize = true
      else if (capitalize) {
        out ++= ch.toUpper.toString
        capitalize = false
      } else out += ch
    }
    out.result()
  }

  private def mangle(input: String): String =
    input.map(c => if (c.isLetterOrDigit) c else '_')

  // ── Type expression analysis ───────────────────────────────────────────────

  private def parseType(expr: String): Option[Type] =
    dialects.Scala3(expr).parse[Type].toOption

  private def lastNameOf(tpe: Type): Option[String] =
    tpe match {
      case Type.Name(n)                    => Some(n)
      case Type.Select(_, Type.Name(n))    => Some(n)
      case Type.Apply.After_4_6_0(base, _) => lastNameOf(base)
      case _                               => None
    }

  private def lastTypeName(expr: String): Option[String] =
    parseType(expr).flatMap(lastNameOf)

  private def isFutureName(name: String): Boolean =
    name == "Future"

  /** Unwrap `Future[T]` into `T`; returns the input when it is not a Future. */
  private def unwrapFuture(tpe: Type): Type =
    tpe match {
      case Type.Apply.After_4_6_0(base, args) if lastNameOf(base).exists(isFutureName) && args.values.size == 1 =>
        args.values.head
      case _ => tpe
    }

  private def isUnitType(tpe: Type): Boolean =
    lastNameOf(tpe).contains("Unit") || tpe.syntax == "Unit"

  // ── Canonical surface helpers (port of the Rust client macro helpers) ─────

  private def isRootMethod(tool: Tool, m: Method): Boolean =
    kebabCase(m.name) == tool.toolName

  private def rootMethodOf(tool: Tool): Option[Method] =
    tool.methods.find(isRootMethod(tool, _))

  private def commandNameOf(tool: Tool, m: Method): String =
    if (isRootMethod(tool, m)) tool.toolName
    else m.nameOverride.getOrElse(kebabCase(m.name))

  private def paramSurfaces(p: Param): List[String] =
    (p.kebab :: p.aliases).distinct

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

  private def isGlobalParam(p: Param): Boolean =
    p.scope.contains("global")

  private def isFlagParam(p: Param): Boolean =
    p.kind.exists(k => k == "flag" || k == "count-flag") ||
      lastTypeName(p.typeExpr).contains("Boolean")

  private def isCountFlag(p: Param): Boolean =
    p.kind.contains("count-flag")

  private def isStreamParam(p: Param): Boolean =
    p.isStdin || p.isStdout

  /**
   * The root global parameters a non-root command inherits into its client
   * signature: every root-method `scope = "global"` parameter whose surface
   * names do not collide with one of the command's own parameters.
   */
  private def inheritedRootParams(tool: Tool, m: Method): List[Param] =
    if (isRootMethod(tool, m)) Nil
    else
      rootMethodOf(tool) match {
        case None       => Nil
        case Some(root) =>
          root.params.filter(isGlobalParam).filterNot { rootParam =>
            m.params.exists { own =>
              surfacesIntersect(rootParam.kebab, rootParam.aliases, own.kebab, own.aliases)
            }
          }
      }

  /**
   * The canonical input field name of one client parameter: its own kebab name,
   * unless a non-root command parameter shadows a root global — then the root
   * global's name (the canonical model dedupes onto the global).
   */
  private def canonicalValueName(tool: Tool, m: Method, p: Param): String =
    if (isRootMethod(tool, m)) p.kebab
    else
      rootMethodOf(tool).flatMap { root =>
        root.params.find { rootParam =>
          isGlobalParam(rootParam) &&
          surfacesIntersect(rootParam.kebab, rootParam.aliases, p.kebab, p.aliases)
        }
      }
        .map(_.kebab)
        .getOrElse(p.kebab)

  private def canonicalAliases(tool: Tool, m: Method, p: Param): List[String] =
    rootMethodOf(tool).flatMap { root =>
      root.params.find { rootParam =>
        isGlobalParam(rootParam) &&
        surfacesIntersect(rootParam.kebab, rootParam.aliases, p.kebab, p.aliases)
      }
    }
      .map(_.aliases)
      .getOrElse(p.aliases)

  /** Whether an omitted canonical surface name covers this parameter. */
  private def omittedMatches(tool: Tool, m: Method, p: Param, omitted: List[String]): Boolean =
    paramSurfaces(p).exists(omitted.contains) || {
      !isRootMethod(tool, m) &&
      rootMethodOf(tool).exists { root =>
        root.params.exists { rootParam =>
          isGlobalParam(rootParam) &&
          paramSurfaces(rootParam).exists(omitted.contains) &&
          surfacesIntersect(rootParam.kebab, rootParam.aliases, p.kebab, p.aliases)
        }
      }
    }

  /**
   * The canonical surfaces a subtree navigation supplies to the child: the
   * already-omitted set plus every surface of the inherited root globals and of
   * the subtree method's own (non-injected) parameters.
   */
  private def childOmittedSurfaces(
    tool: Tool,
    m: Method,
    inheritedOmitted: List[String]
  ): List[String] = {
    val out = mutable.LinkedHashSet.empty[String]
    inheritedOmitted.foreach(out.add)
    inheritedRootParams(tool, m)
      .filterNot(p => omittedMatches(tool, m, p, inheritedOmitted))
      .foreach(p => paramSurfaces(p).foreach(out.add))
    m.params.filterNot(p => p.isPrincipal || isStreamParam(p) || omittedMatches(tool, m, p, inheritedOmitted)).foreach {
      p =>
        out.add(canonicalValueName(tool, m, p))
        canonicalAliases(tool, m, p).foreach(out.add)
    }
    out.toList
  }

  // ── Return shape resolution ────────────────────────────────────────────────

  private final class FileGenerator(
    root: Tool,
    allTools: List[Tool],
    warnings: mutable.Builder[Warning, List[Warning]]
  ) {
    private val clientName = s"${root.name}Client"

    private val descriptorVals  = mutable.LinkedHashMap.empty[String, String] // valName -> trait type ref
    private val modelVals       =
      mutable.LinkedHashMap.empty[String, (String, List[String])] // valName -> (descriptorVal, schemaPath)
    private val errorSchemaVals = mutable.LinkedHashMap.empty[String, String] // valName -> error type expr
    private val wrapperDefs     = mutable.ListBuffer.empty[String]

    private def traitTypeRef(tool: Tool): String =
      if (tool.pkg.isEmpty) tool.name
      else if (tool.pkg == root.pkg) tool.name
      else s"_root_.${tool.pkg}.${tool.name}"

    private def descriptorVal(tool: Tool): String = {
      val valName = s"__descriptor_${mangle(tool.fqn)}"
      descriptorVals.getOrElseUpdate(valName, traitTypeRef(tool))
      valName
    }

    private def modelVal(tool: Tool, contextId: String, m: Method, schemaPath: List[String]): String = {
      val valName = s"__model_${mangle(if (contextId.isEmpty) m.name else s"${contextId}_${m.name}")}"
      modelVals.getOrElseUpdate(valName, (descriptorVal(tool), schemaPath))
      valName
    }

    private def errorSchemaVal(errType: String): String = {
      val valName = s"__errorSchema_${mangle(errType)}"
      errorSchemaVals.getOrElseUpdate(valName, errType)
      valName
    }

    /** Resolve a return type expression to a discovered tool trait, if any. */
    private def resolveToolTrait(tpe: Type): Option[Tool] = {
      def resolve(simpleName: String, qualified: Option[String]): Option[Tool] =
        qualified match {
          case Some(fqn) => allTools.find(_.fqn == fqn)
          case None      =>
            allTools.find(t => t.name == simpleName && t.pkg == root.pkg).orElse {
              allTools.filter(_.name == simpleName) match {
                case single :: Nil => Some(single)
                case _             => None
              }
            }
        }
      tpe match {
        case Type.Name(n)   => resolve(n, None)
        case t: Type.Select => resolve(t.name.value, Some(t.syntax))
        case _              => None
      }
    }

    private def returnShapeOf(m: Method): ReturnShape =
      parseType(m.returnTypeExpr) match {
        case None      => LeafReturn(Some(m.returnTypeExpr), None)
        case Some(tpe) =>
          resolveToolTrait(tpe) match {
            case Some(child) => SubtreeReturn(child)
            case None        =>
              val inner = unwrapFuture(tpe)
              inner match {
                case Type.Apply.After_4_6_0(base, args)
                    if lastNameOf(base).contains("Either") && args.values.size == 2 =>
                  val err = args.values.head
                  val ok  = args.values(1)
                  LeafReturn(
                    okType = if (isUnitType(ok)) None else Some(ok.syntax),
                    errType = Some(err.syntax)
                  )
                case other =>
                  LeafReturn(
                    okType = if (isUnitType(other)) None else Some(other.syntax),
                    errType = None
                  )
              }
          }
      }

    // ── Rendering ────────────────────────────────────────────────────────────

    private def paramDecl(p: Param): String = {
      val tpe =
        if (p.isStdin) "_root_.golem.tool.ToolInputStream"
        else if (p.isStdout) "_root_.golem.tool.ToolOutputStream"
        else p.typeExpr
      s"${p.ident}: $tpe"
    }

    private def keptLeafParams(tool: Tool, m: Method, omitted: List[String]): List[Param] =
      (inheritedRootParams(tool, m) ++ m.params).filter { p =>
        !p.isPrincipal && !p.isStdout && !omittedMatches(tool, m, p, omitted)
      }

    private def keptSubtreeParams(tool: Tool, m: Method, omitted: List[String]): List[Param] =
      (inheritedRootParams(tool, m) ++ m.params).filter { p =>
        !p.isPrincipal && !omittedMatches(tool, m, p, omitted)
      }

    private def okResultType(okType: Option[String], hasStdout: Boolean): String =
      (okType, hasStdout) match {
        case (Some(ok), true)  => s"($ok, _root_.golem.tool.ToolOutputStream)"
        case (None, true)      => "_root_.golem.tool.ToolOutputStream"
        case (Some(ok), false) => ok
        case (None, false)     => "_root_.scala.Unit"
      }

    private def leafReturnType(shape: LeafReturn, hasStdout: Boolean): String = {
      val err = shape.errType.getOrElse("_root_.scala.Nothing")
      val ok  = okResultType(shape.okType, hasStdout)
      s"_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[$err], $ok]]"
    }

    private def valueEntry(tool: Tool, m: Method, p: Param): String = {
      val name = canonicalValueName(tool, m, p)
      if (isCountFlag(p))
        s"""("$name", _root_.golem.tool.ToolClientRuntime.countFlagValue(${p.ident}))"""
      else
        s"""("$name", _root_.scala.Predef.implicitly[_root_.golem.schema.IntoSchema[${p.typeExpr}]].toValue(${p.ident}))"""
    }

    private def prefixEntry(tool: Tool, m: Method, p: Param): String = {
      val name        = canonicalValueName(tool, m, p)
      val aliases     = canonicalAliases(tool, m, p)
      val aliasesExpr =
        if (aliases.isEmpty) "_root_.scala.Nil"
        else aliases.map(a => s""""$a"""").mkString("_root_.scala.List(", ", ", ")")
      if (isCountFlag(p))
        s"""_root_.golem.tool.ToolClientRuntime.countFlagPrefixValue("$name", $aliasesExpr, ${p.ident})"""
      else
        s"""_root_.golem.tool.ToolClientRuntime.prefixValue("$name", $aliasesExpr, ${p.ident}, _root_.scala.Predef.implicitly[_root_.golem.schema.IntoSchema[${p.typeExpr}]])"""
    }

    private def listExpr(entries: List[String], indent: String): String =
      if (entries.isEmpty) "_root_.scala.Nil"
      else
        entries.mkString(s"_root_.scala.List(\n$indent  ", s",\n$indent  ", s"\n$indent)")

    private def stringListExpr(entries: List[String]): String =
      if (entries.isEmpty) "_root_.scala.Nil"
      else entries.map(e => s""""$e"""").mkString("_root_.scala.List(", ", ", ")")

    /**
     * Renders one leaf command method. `contextId` is empty for the root
     * client, otherwise the wrapper path (used for cache val naming);
     * `isWrapper` selects the dynamic (inherited-prefix) input path.
     */
    private def leafMethod(
      tool: Tool,
      m: Method,
      shape: LeafReturn,
      omitted: List[String],
      contextId: String,
      isWrapper: Boolean,
      indent: String
    ): String = {
      val kept      = keptLeafParams(tool, m, omitted)
      val hasStdout = m.params.exists(_.isStdout)
      val stdin     = m.params.find(_.isStdin)
      val retType   = leafReturnType(shape, hasStdout)

      val commandName = commandNameOf(tool, m)
      val isBody      = commandName == tool.toolName
      val schemaPath  = if (isBody) Nil else List(commandName)

      val commandPathExpr =
        if (isWrapper) {
          if (isBody) "__commandPath" else s"""__commandPath :+ "$commandName""""
        } else {
          if (isBody) "_root_.scala.Nil" else s"""_root_.scala.List("$commandName")"""
        }

      val valueEntries = kept
        .filterNot(isStreamParam)
        .map(valueEntry(tool, m, _))

      val model = modelVal(tool, contextId, m, schemaPath)

      val inputExpr =
        if (isWrapper) {
          val desc = descriptorVal(tool)
          s"""if (__inheritedPrefix.isEmpty)
$indent      _root_.golem.tool.ToolClientRuntime.buildInputFromModel($model, __values)
$indent    else
$indent      _root_.golem.tool.ToolClientRuntime.buildDynamicInput($desc, ${stringListExpr(
              schemaPath
            )}, __inheritedPrefix, __values)"""
        } else
          s"_root_.golem.tool.ToolClientRuntime.buildInputFromModel($model, __values)"

      val stdinExpr = stdin.map(p => s"_root_.scala.Some(${p.ident})").getOrElse("_root_.scala.None")

      val runExpr = shape.errType match {
        case Some(err) =>
          val schema = errorSchemaVal(err)
          s"_root_.golem.tool.ToolClientRuntime.run[$err](__transport, $commandPathExpr, __input, $stdinExpr, $schema.fromErrorPayloadValue(_))"
        case None =>
          s"_root_.golem.tool.ToolClientRuntime.runInfallible(__transport, $commandPathExpr, __input, $stdinExpr)"
      }

      val decodeExpr = (shape.okType, hasStdout) match {
        case (Some(ok), true) =>
          s"_root_.golem.tool.ToolClientRuntime.decodeValueStdoutResult(__r, _root_.scala.Predef.implicitly[_root_.golem.schema.FromSchema[$ok]])"
        case (None, true) =>
          "_root_.golem.tool.ToolClientRuntime.decodeStdoutResult(__r)"
        case (Some(ok), false) =>
          s"_root_.golem.tool.ToolClientRuntime.decodeValueResult(__r, _root_.scala.Predef.implicitly[_root_.golem.schema.FromSchema[$ok]])"
        case (None, false) =>
          "_root_.golem.tool.ToolClientRuntime.decodeUnitResult(__r)"
      }

      val paramDecls = kept.map(paramDecl).mkString(", ")

      s"""${indent}def ${m.name}($paramDecls): $retType = {
$indent  val __params = _root_.golem.tool.ToolClientRuntime.encodeParams(${listExpr(valueEntries, s"$indent ")})
$indent  val __input = __params.flatMap { __values =>
$indent    $inputExpr
$indent  }
$indent  _root_.golem.tool.ToolClientRuntime.complete(
$indent    $runExpr
$indent  )(__r => $decodeExpr)
$indent}"""
    }

    /** Renders one subtree navigation method and its child wrapper class. */
    private def subtreeMethod(
      tool: Tool,
      m: Method,
      child: Tool,
      omitted: List[String],
      pathClasses: List[String],
      visited: Set[String],
      isWrapper: Boolean,
      indent: String
    ): Option[String] = {
      if (visited.contains(child.fqn)) {
        warnings += Warning(
          s"Skipping subtree client method ${tool.fqn}.${m.name}: subtree cycle through ${child.fqn}."
        )
        return None
      }

      val kept = keptSubtreeParams(tool, m, omitted)
      // Subtree navigation always pushes the method's own command name (the
      // implicit-body method cannot be a subtree method, so the tool-name
      // special case never applies here).
      val commandName = m.nameOverride.getOrElse(kebabCase(m.name))
      val wrapperName = (pathClasses :+ pascalCase(m.name)).mkString + "Client"

      // Prefix packing order mirrors the Rust client: inherited globals then
      // own parameters, each group with flags after non-flags.
      val prefixParams = {
        val inherited = inheritedRootParams(tool, m).sortBy(p => if (isFlagParam(p)) 1 else 0)
        val own       = m.params.sortBy(p => if (isFlagParam(p)) 1 else 0)
        (inherited ++ own).filter { p =>
          !p.isPrincipal && !isStreamParam(p) && !omittedMatches(tool, m, p, omitted)
        }
      }
      val prefixEntries = prefixParams.map(prefixEntry(tool, m, _))

      val basePrefix = if (isWrapper) "__inheritedPrefix ++ " else ""
      val prefixExpr =
        if (prefixEntries.isEmpty) {
          if (isWrapper) "__inheritedPrefix" else "_root_.scala.Nil"
        } else s"$basePrefix${listExpr(prefixEntries, s"$indent ")}"
      val commandPathExpr =
        if (isWrapper) s"""__commandPath :+ "$commandName""""
        else s"""_root_.scala.List("$commandName")"""

      val childOmitted = childOmittedSurfaces(tool, m, omitted)
      generateWrapper(child, childOmitted, pathClasses :+ pascalCase(m.name), visited + child.fqn)

      val paramDecls = kept.map(paramDecl).mkString(", ")

      Some(
        s"""${indent}def ${m.name}($paramDecls): $clientName.$wrapperName = {
$indent  val __prefix = $prefixExpr
$indent  new $clientName.$wrapperName(
$indent    _root_.golem.runtime.tool.client.ToolRpcClient.transport($clientName.toolName),
$indent    $commandPathExpr,
$indent    __prefix
$indent  )
$indent}"""
      )
    }

    /**
     * Renders the abstract signature of one method for the root client trait.
     */
    private def traitSignature(tool: Tool, m: Method): Option[String] =
      returnShapeOf(m) match {
        case SubtreeReturn(child) =>
          if (allVisited.contains(child.fqn)) None
          else {
            val kept        = keptSubtreeParams(tool, m, Nil)
            val wrapperName = pascalCase(m.name) + "Client"
            Some(s"  def ${m.name}(${kept.map(paramDecl).mkString(", ")}): $clientName.$wrapperName")
          }
        case shape: LeafReturn =>
          val kept      = keptLeafParams(tool, m, Nil)
          val hasStdout = m.params.exists(_.isStdout)
          Some(s"  def ${m.name}(${kept.map(paramDecl).mkString(", ")}): ${leafReturnType(shape, hasStdout)}")
      }

    /** Trait fqns whose subtree methods were cut because of a cycle. */
    private val allVisited = mutable.Set.empty[String]

    private def generateWrapper(
      tool: Tool,
      omitted: List[String],
      pathClasses: List[String],
      visited: Set[String]
    ): Unit = {
      val wrapperName = pathClasses.mkString + "Client"
      val methods     = tool.methods.flatMap { m =>
        returnShapeOf(m) match {
          case SubtreeReturn(child) =>
            subtreeMethod(tool, m, child, omitted, pathClasses, visited, isWrapper = true, indent = "    ")
          case shape: LeafReturn =>
            Some(
              leafMethod(
                tool,
                m,
                shape,
                omitted,
                contextId = pathClasses.mkString,
                isWrapper = true,
                indent = "    "
              )
            )
        }
      }

      wrapperDefs += s"""  final class $wrapperName private[$clientName] (
    __transport: _root_.golem.tool.ToolRpcTransport,
    __commandPath: _root_.scala.List[_root_.scala.Predef.String],
    __inheritedPrefix: _root_.scala.List[_root_.golem.tool.CanonicalInputValue]
  ) {
${methods.mkString("\n\n")}
  }"""
    }

    def generate(): String = {
      // Render root method impls first so cache vals and wrappers are collected.
      val rootImpls = root.methods.flatMap { m =>
        returnShapeOf(m) match {
          case SubtreeReturn(child) =>
            subtreeMethod(
              tool = root,
              m = m,
              child = child,
              omitted = Nil,
              pathClasses = Nil,
              visited = Set(root.fqn),
              isWrapper = false,
              indent = "    "
            ) match {
              case Some(impl) => Some(impl)
              case None       =>
                allVisited.add(child.fqn)
                None
            }
          case shape: LeafReturn =>
            Some(leafMethod(root, m, shape, Nil, contextId = "", isWrapper = false, indent = "    "))
        }
      }

      val signatures = root.methods.flatMap(traitSignature(root, _))

      val sb = new StringBuilder

      if (root.pkg.nonEmpty) {
        sb.append(s"package ${root.pkg}\n\n")
      }

      sb.append("/** Generated by Golem tool RPC codegen. Do not edit. */\n")
      sb.append(s"trait $clientName {\n")
      signatures.foreach(s => sb.append(s + "\n"))
      sb.append("}\n\n")

      sb.append(s"object $clientName {\n\n")
      sb.append(s"""  val toolName: _root_.scala.Predef.String = "${root.toolName}"\n\n""")
      sb.append(s"  def apply(): $clientName = new Root()\n\n")

      descriptorVals.foreach { case (valName, traitRef) =>
        sb.append(
          s"  private lazy val $valName: _root_.scala.Either[_root_.golem.tool.ToolBuildError, _root_.golem.tool.ExtendedToolType] =\n"
        )
        sb.append(s"    _root_.golem.runtime.macros.ToolDefinitionMacro.tryMetadata[$traitRef]\n\n")
      }

      modelVals.foreach { case (valName, (descriptor, schemaPath)) =>
        sb.append(
          s"  private lazy val $valName: _root_.scala.Either[_root_.scala.Predef.String, _root_.golem.tool.CanonicalInputModel] =\n"
        )
        sb.append(
          s"    _root_.golem.tool.ToolClientRuntime.staticInputModel($descriptor, ${stringListExpr(schemaPath)})\n\n"
        )
      }

      errorSchemaVals.foreach { case (valName, errType) =>
        sb.append(s"  private lazy val $valName: _root_.golem.tool.ToolErrorSchema[$errType] =\n")
        sb.append(s"    _root_.golem.runtime.macros.ToolErrorSchemaDerivation.derive[$errType]\n\n")
      }

      sb.append(s"  private final class Root extends $clientName {\n")
      sb.append(
        "    private val __transport: _root_.golem.tool.ToolRpcTransport =\n" +
          "      _root_.golem.runtime.tool.client.ToolRpcClient.transport(toolName)\n\n"
      )
      sb.append(rootImpls.mkString("\n\n"))
      sb.append("\n  }\n")

      wrapperDefs.foreach { w =>
        sb.append("\n")
        sb.append(w)
        sb.append("\n")
      }

      sb.append("}\n")
      sb.toString
    }
  }
}
