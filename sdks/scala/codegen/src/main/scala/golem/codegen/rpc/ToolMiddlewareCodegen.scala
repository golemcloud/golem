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
import golem.codegen.rpc.ToolProjectionRendering.InvocationUnderlying

import scala.collection.mutable

/** Generates nominal typed-underlying and middleware authoring projections. */
object ToolMiddlewareCodegen {

  final case class GeneratedFile(relativePath: String, content: String)

  final case class Error(path: Option[String], message: String)

  final case class Result(files: Seq[GeneratedFile], errors: Seq[Error])

  def generate(
    tools: List[Tool],
    existingObjects: Seq[SourceDiscovery.ExistingObject]
  ): Result = {
    val toolsByFqn    = tools.map(tool => tool.fqn -> tool).toMap
    val existingByFqn = existingObjects.map { obj =>
      if (obj.pkg.isEmpty) obj.name else s"${obj.pkg}.${obj.name}"
    }.toSet
    val errors = List.newBuilder[Error]
    val files  = List.newBuilder[GeneratedFile]

    tools.foreach { tool =>
      val middlewareName = s"${tool.name}Middleware"
      val underlyingName = s"${tool.name}Underlying"
      val middlewareFqn  = qualify(tool.pkg, middlewareName)
      val underlyingFqn  = qualify(tool.pkg, underlyingName)

      if (existingByFqn.contains(middlewareFqn))
        errors += Error(
          Some(tool.path),
          s"Cannot generate $middlewareFqn because an object with that name already exists."
        )
      else if (existingByFqn.contains(underlyingFqn))
        errors += Error(
          Some(tool.path),
          s"Cannot generate $underlyingFqn because an object with that name already exists."
        )
      else {
        val packagePath     = if (tool.pkg.isEmpty) "" else tool.pkg.replace('.', '/') + "/"
        val requiredImports = ToolProjectionIR
          .reachableTools(tool, toolsByFqn)
          .filter(_.middlewareImportsRequired)
          .flatMap(_.projectionImports)
          .distinct
        files += GeneratedFile(
          s"$packagePath$middlewareName.scala",
          new Renderer(tool, requiredImports).render()
        )
      }
    }

    Result(files.result(), errors.result())
  }

  private def qualify(pkg: String, name: String): String =
    if (pkg.isEmpty) name else s"$pkg.$name"

  private final class Renderer(tool: Tool, requiredImports: List[String]) {
    private val middlewareName = s"${tool.name}Middleware"
    private val underlyingName = s"${tool.name}Underlying"

    private val modelVals = mutable.LinkedHashMap.empty[String, List[String]]
    private val errorVals = mutable.LinkedHashMap.empty[String, String]

    private def mangle(input: String): String =
      input.map(char => if (char.isLetterOrDigit) char else '_')

    private def paramDecl(projected: ProjectedParam): String =
      ToolProjectionRendering.paramDecl(projected, InvocationUnderlying)

    private def methodSignature(
      leaf: FlattenedLeaf,
      params: List[ProjectedParam],
      underlyingType: Option[String],
      indent: String
    ): String = {
      val declarations = underlyingType.toList.map(tpe => s"underlying: $tpe") ++ params.map(paramDecl)
      s"${indent}def ${leaf.name}(${declarations.mkString(", ")}): ${ToolProjectionRendering.returnType(leaf.codec, InvocationUnderlying)}"
    }

    private def listExpr(entries: List[String], indent: String): String =
      if (entries.isEmpty) "_root_.scala.Nil"
      else entries.mkString(s"_root_.scala.List(\n$indent  ", s",\n$indent  ", s"\n$indent)")

    private def modelVal(leaf: FlattenedLeaf): String = {
      val name = s"__model_${mangle(leaf.name)}"
      modelVals.getOrElseUpdate(name, leaf.commandPath)
      name
    }

    private def errorVal(errorType: String): String = {
      val name = s"__errorSchema_${mangle(errorType)}"
      errorVals.getOrElseUpdate(name, errorType)
      name
    }

    private def implementation(leaf: FlattenedLeaf): String = {
      val params       = leaf.underlyingParams
      val valueEntries = params
        .filterNot(projected => isStreamParam(projected.param))
        .map(ToolProjectionRendering.valueEntry(_, InvocationUnderlying))
      val stdin = params
        .find(_.param.isStdin)
        .map(projected => s"_root_.scala.Some(${projected.param.ident})")
        .getOrElse("_root_.scala.None")
      val model       = modelVal(leaf)
      val errorSchema = leaf.codec.projectedErrType.map(errorVal)
      val run         = ToolProjectionRendering.runExpression(
        InvocationUnderlying,
        leaf.codec,
        "__golemRawUnderlying",
        ToolProjectionRendering.stringList(leaf.commandPath),
        "__golemInput",
        stdin,
        errorSchema,
        Some("__descriptor")
      )
      val decode = ToolProjectionRendering.decodeExpression(
        InvocationUnderlying,
        leaf.codec,
        "__golemResult"
      )

      s"""${methodSignature(leaf, params, None, "    ")} = {
      val __golemParams = _root_.golem.tool.ToolUnderlyingRuntime.encodeParams(${listExpr(valueEntries, "      ")})
      val __golemInput = __golemParams.flatMap(__golemValues =>
        _root_.golem.tool.ToolUnderlyingRuntime.buildInputFromModel($model, __golemValues)
      )
      _root_.golem.tool.ToolUnderlyingRuntime.complete(
        $run
      )(__golemResult => $decode)
    }"""
    }

    def render(): String = {
      val implementations = tool.flattenedLeaves.map(implementation)
      val sb              = new StringBuilder

      if (tool.pkg.nonEmpty) sb.append(s"package ${tool.pkg}\n\n")
      if (requiredImports.nonEmpty) {
        requiredImports.foreach(importStatement => sb.append(importStatement).append("\n"))
        sb.append("\n")
      }
      sb.append("/** Generated by Golem tool middleware codegen. Do not edit. */\n")
      sb.append(s"trait $underlyingName {\n")
      tool.flattenedLeaves.foreach { leaf =>
        sb.append(methodSignature(leaf, leaf.underlyingParams, None, "  ")).append("\n")
      }
      sb.append("}\n\n")

      sb.append(s"object $underlyingName {\n")
      sb.append(s"""  val __golemToolName: _root_.scala.Predef.String = "${tool.toolName}"\n""")
      sb.append(s"""  val __golemToolType: _root_.scala.Predef.String = "${tool.fqn}"\n\n""")
      sb.append(
        s"  private lazy val __descriptor: _root_.scala.Either[_root_.golem.tool.ToolBuildError, _root_.golem.tool.ExtendedToolType] =\n"
      )
      sb.append(s"    _root_.golem.runtime.macros.ToolDefinitionMacro.tryMetadata[${tool.name}]\n\n")
      modelVals.foreach { case (name, commandPath) =>
        sb.append(
          s"  private lazy val $name: _root_.scala.Either[_root_.scala.Predef.String, _root_.golem.tool.CanonicalInputModel] =\n"
        )
        sb.append(
          s"    _root_.golem.tool.ToolUnderlyingRuntime.staticInputModel(__descriptor, ${ToolProjectionRendering.stringList(commandPath)})\n\n"
        )
      }
      errorVals.foreach { case (name, errorType) =>
        sb.append(s"  private lazy val $name: _root_.golem.tool.ToolErrorSchema[$errorType] =\n")
        sb.append(s"    _root_.golem.runtime.macros.ToolErrorSchemaDerivation.derive[$errorType]\n\n")
      }
      sb.append(
        s"  def __golemFromRaw(underlying: _root_.golem.tool.RawToolUnderlying): $underlyingName =\n"
      )
      sb.append("    new Impl(underlying)\n\n")
      sb.append(
        s"  private final class Impl(__golemRawUnderlying: _root_.golem.tool.RawToolUnderlying) extends $underlyingName {\n"
      )
      sb.append(implementations.mkString("\n\n"))
      sb.append("\n  }\n")
      sb.append("}\n\n")

      sb.append(s"trait $middlewareName extends $middlewareName.Adapter[$underlyingName]\n\n")
      sb.append(s"object $middlewareName {\n")
      sb.append(s"""  val __golemPresentedToolType: _root_.scala.Predef.String = "${tool.fqn}"\n\n""")
      sb.append("  trait Adapter[U] {\n")
      tool.flattenedLeaves.foreach { leaf =>
        sb.append(methodSignature(leaf, leaf.params, Some("U"), "    ")).append("\n")
      }
      sb.append("  }\n")
      sb.append("}\n")
      sb.toString
    }
  }
}
