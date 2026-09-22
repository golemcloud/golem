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
import golem.codegen.rpc.ToolProjectionIR._
import golem.codegen.rpc.ToolProjectionRendering.AmbientClient

import scala.collection.mutable

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
    warnings: Seq[Warning],
    errors: Seq[ToolProjectionIR.Error] = Nil
  )

  def generate(
    tools: List[SourceDiscovery.ToolTrait],
    existingObjects: Seq[SourceDiscovery.ExistingObject]
  ): Result = {
    val projection = ToolProjectionIR.build(tools)
    generateFromIR(projection.tools, existingObjects).copy(errors = projection.errors)
  }

  private[codegen] def generateFromIR(
    surfaces: List[Tool],
    existingObjects: Seq[SourceDiscovery.ExistingObject]
  ): Result = {
    val warnings = List.newBuilder[Warning]
    val files    = List.newBuilder[GeneratedFile]

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

  // ── Return shape resolution ────────────────────────────────────────────────

  private final class FileGenerator(
    root: Tool,
    allTools: List[Tool],
    warnings: mutable.Builder[Warning, List[Warning]]
  ) {
    private val clientName = s"${root.name}Client"

    private val projectionName  = s"${root.name}CallProjection"
    private val wrapperDefs     = mutable.ListBuffer.empty[String]
    private val toolsByFqn      = allTools.map(tool => tool.fqn -> tool).toMap
    private val requiredImports = ToolProjectionIR
      .reachableTools(root, toolsByFqn)
      .filter(_.ambientImportsRequired)
      .flatMap(_.projectionImports)
      .distinct

    // ── Rendering ────────────────────────────────────────────────────────────

    private def paramDecl(param: Param): String =
      s"${param.ident}: ${ToolProjectionRendering.paramType(param, AmbientClient)}"

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
        case (Some(ok), true)  => ok
        case (None, true)      => "_root_.scala.Unit"
        case (Some(ok), false) => ok
        case (None, false)     => "_root_.scala.Unit"
      }

    private def leafReturnType(shape: LeafReturn, hasStdout: Boolean): String = {
      val err = shape.errType.getOrElse("_root_.scala.Nothing")
      val ok  = okResultType(shape.okType, hasStdout)
      if (hasStdout)
        s"_root_.scala.Either[_root_.golem.tool.ToolError[$err], _root_.golem.tool.ToolInvocation[$err, $ok]]"
      else
        s"_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[$err], $ok]]"
    }

    private def valueEntry(tool: Tool, method: Method, param: Param): String =
      ToolProjectionRendering.valueEntry(projected(tool, method, param), AmbientClient)

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

    /** Renders one leaf command method. */
    private def leafMethod(
      tool: Tool,
      m: Method,
      shape: LeafReturn,
      omitted: List[String],
      isWrapper: Boolean,
      indent: String
    ): String = {
      val kept    = keptLeafParams(tool, m, omitted)
      val stdin   = m.params.find(_.isStdin)
      val retType = leafReturnType(shape, shape.hasStdout)

      val valueEntries = kept
        .filterNot(isStreamParam)
        .map(valueEntry(tool, m, _))

      val stdinExpr = stdin.map(p => s"_root_.scala.Some(${p.ident})").getOrElse("_root_.scala.None")

      val paramDecls = kept.map(paramDecl).mkString(", ")
      val prefixExpr = if (isWrapper) "__inheritedPrefix" else "_root_.scala.Nil"
      val operation  = if (shape.hasStdout) "__start" else "__await"

      s"""${indent}def ${m.name}($paramDecls): $retType = {
$indent  val __params = _root_.golem.tool.ToolCallPreparation.encodeParams(${listExpr(valueEntries, s"$indent ")})
$indent  $projectionName.${operation}_${m.name}(__backend, $prefixExpr, __params, $stdinExpr)
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
      val childOmitted = childOmittedSurfaces(tool, m, omitted)
      generateWrapper(child, childOmitted, pathClasses :+ pascalCase(m.name), visited + child.fqn)

      val paramDecls = kept.map(paramDecl).mkString(", ")

      Some(
        s"""${indent}def ${m.name}($paramDecls): $clientName.$wrapperName = {
$indent  val __prefix = $prefixExpr
$indent  new $clientName.$wrapperName(
$indent    __backend,
$indent    __prefix
$indent  )
$indent}"""
      )
    }

    /**
     * Renders the abstract signature of one method for the root client trait.
     */
    private def traitSignature(tool: Tool, m: Method): Option[String] =
      m.returnShape match {
        case SubtreeReturn(childFqn) =>
          val child = toolsByFqn(childFqn)
          if (allVisited.contains(child.fqn)) None
          else {
            val kept        = keptSubtreeParams(tool, m, Nil)
            val wrapperName = pascalCase(m.name) + "Client"
            Some(s"  def ${m.name}(${kept.map(paramDecl).mkString(", ")}): $clientName.$wrapperName")
          }
        case shape: LeafReturn =>
          val kept = keptLeafParams(tool, m, Nil)
          Some(
            s"  def ${m.name}(${kept.map(paramDecl).mkString(", ")}): ${leafReturnType(shape, shape.hasStdout)}"
          )
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
        m.returnShape match {
          case SubtreeReturn(childFqn) =>
            val child = toolsByFqn(childFqn)
            subtreeMethod(tool, m, child, omitted, pathClasses, visited, isWrapper = true, indent = "    ")
          case shape: LeafReturn =>
            Some(
              leafMethod(
                tool,
                m,
                shape,
                omitted,
                isWrapper = true,
                indent = "    "
              )
            )
        }
      }

      wrapperDefs += s"""  final class $wrapperName private[$clientName] (
    __backend: _root_.golem.tool.AmbientToolCallBackend,
    __inheritedPrefix: _root_.scala.List[_root_.golem.tool.CanonicalInputValue]
  ) {
${methods.mkString("\n\n")}
  }"""
    }

    def generate(): String = {
      // Render root method impls first so cache vals and wrappers are collected.
      val rootImpls = root.methods.flatMap { m =>
        m.returnShape match {
          case SubtreeReturn(childFqn) =>
            val child = toolsByFqn(childFqn)
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
            Some(leafMethod(root, m, shape, Nil, isWrapper = false, indent = "    "))
        }
      }

      val signatures = root.methods.flatMap(traitSignature(root, _))

      val sb = new StringBuilder

      if (root.pkg.nonEmpty) {
        sb.append(s"package ${root.pkg}\n\n")
      }
      if (requiredImports.nonEmpty) {
        requiredImports.foreach(importStatement => sb.append(importStatement).append("\n"))
        sb.append("\n")
      }

      sb.append("/** Generated by Golem tool RPC codegen. Do not edit. */\n")
      sb.append(s"trait $clientName {\n")
      signatures.foreach(s => sb.append(s + "\n"))
      sb.append("}\n\n")

      sb.append(s"object $clientName {\n\n")
      sb.append(s"""  val toolName: _root_.scala.Predef.String = "${root.toolName}"\n\n""")
      sb.append(s"  def apply(): $clientName = apply(toolName)\n\n")
      sb.append(s"  def apply(lookupName: _root_.scala.Predef.String): $clientName = new Root(lookupName)\n\n")

      sb.append(s"  private final class Root(lookupName: _root_.scala.Predef.String) extends $clientName {\n")
      sb.append(
        "    private val __backend: _root_.golem.tool.AmbientToolCallBackend =\n" +
          "      new _root_.golem.tool.AmbientToolCallBackend(_root_.golem.runtime.tool.client.ToolRpcClient.transport(lookupName))\n\n"
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
