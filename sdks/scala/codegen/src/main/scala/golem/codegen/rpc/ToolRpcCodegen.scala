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

  private def mangle(input: String): String =
    input.map(c => if (c.isLetterOrDigit) c else '_')

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
    private val toolsByFqn      = allTools.map(tool => tool.fqn -> tool).toMap
    private val requiredImports = ToolProjectionIR
      .reachableTools(root, toolsByFqn)
      .filter(_.ambientImportsRequired)
      .flatMap(_.projectionImports)
      .distinct

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

    // ── Rendering ────────────────────────────────────────────────────────────

    private def paramDecl(param: Param): String =
      s"${param.ident}: ${ToolProjectionRendering.paramType(param, AmbientClient)}"

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
      val kept    = keptLeafParams(tool, m, omitted)
      val stdin   = m.params.find(_.isStdin)
      val retType = ToolProjectionRendering.returnType(shape, AmbientClient)

      val schemaPath = m.localCommandPath

      val commandPathExpr = ToolProjectionRendering.commandPath(
        if (isWrapper) Some("__commandPath") else None,
        m.localCommandPath
      )

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
$indent      _root_.golem.tool.ToolClientRuntime.buildDynamicInput($desc, ${ToolProjectionRendering.stringList(
              schemaPath
            )}, __inheritedPrefix, __values)"""
        } else
          s"_root_.golem.tool.ToolClientRuntime.buildInputFromModel($model, __values)"

      val stdinExpr = stdin.map(p => s"_root_.scala.Some(${p.ident})").getOrElse("_root_.scala.None")

      val runExpr = ToolProjectionRendering.runExpression(
        AmbientClient,
        shape,
        "__transport",
        commandPathExpr,
        "__input",
        stdinExpr,
        shape.errType.map(errorSchemaVal)
      )

      val decodeExpr = ToolProjectionRendering.decodeExpression(AmbientClient, shape, "__r")

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
      val commandPathExpr = ToolProjectionRendering.commandPath(
        if (isWrapper) Some("__commandPath") else None,
        m.localCommandPath
      )

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
            s"  def ${m.name}(${kept.map(paramDecl).mkString(", ")}): ${ToolProjectionRendering.returnType(shape, AmbientClient)}"
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
            Some(leafMethod(root, m, shape, Nil, contextId = "", isWrapper = false, indent = "    "))
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
          s"    _root_.golem.tool.ToolClientRuntime.staticInputModel($descriptor, ${ToolProjectionRendering.stringList(schemaPath)})\n\n"
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
