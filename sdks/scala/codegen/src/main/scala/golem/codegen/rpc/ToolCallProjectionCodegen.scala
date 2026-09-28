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
import golem.codegen.rpc.ToolProjectionIR.{FlattenedLeaf, Tool}

import scala.collection.mutable

/**
 * Generates the shared call semantics used by ambient and underlying tool
 * facades.
 */
object ToolCallProjectionCodegen {

  final case class GeneratedFile(relativePath: String, content: String)

  final case class Error(path: Option[String], message: String)

  final case class Result(files: Seq[GeneratedFile], errors: Seq[Error])

  def generate(
    tools: List[Tool],
    existingObjects: Seq[SourceDiscovery.ExistingObject] = Nil
  ): Result = {
    val toolsByFqn    = tools.map(tool => tool.fqn -> tool).toMap
    val existingByFqn = existingObjects.map { obj =>
      if (obj.pkg.isEmpty) obj.name else s"${obj.pkg}.${obj.name}"
    }.toSet
    val errors = List.newBuilder[Error]
    val files  = List.newBuilder[GeneratedFile]
    tools.foreach { tool =>
      val projectionName = s"${tool.name}CallProjection"
      val projectionFqn  = if (tool.pkg.isEmpty) projectionName else s"${tool.pkg}.$projectionName"
      if (existingByFqn.contains(projectionFqn))
        errors += Error(
          Some(tool.path),
          s"Cannot generate $projectionFqn because an object with that name already exists."
        )
      else {
        val packagePath = if (tool.pkg.isEmpty) "" else tool.pkg.replace('.', '/') + "/"
        files += GeneratedFile(
          s"$packagePath$projectionName.scala",
          new Renderer(tool, toolsByFqn).render()
        )
      }
    }
    Result(files.result(), errors.result())
  }

  private final class Renderer(tool: Tool, toolsByFqn: Map[String, Tool]) {
    private val projectionName  = s"${tool.name}CallProjection"
    private val errorVals       = mutable.LinkedHashMap.empty[String, String]
    private val requiredImports = ToolProjectionIR
      .reachableTools(tool, toolsByFqn)
      .flatMap(_.projectionImports)
      .distinct

    private def errorVal(errorType: String): String =
      errorVals.getOrElseUpdate(errorType, s"__errorSchema_${errorVals.size}")

    private def errorType(leaf: FlattenedLeaf): String =
      leaf.codec.projectedErrType.getOrElse("_root_.scala.Nothing")

    private def valueType(leaf: FlattenedLeaf): String =
      leaf.codec.projectedOkType.getOrElse("_root_.scala.Unit")

    private def errors(leaf: FlattenedLeaf): String =
      leaf.codec.projectedErrType match {
        case Some(error) =>
          s"_root_.golem.tool.ToolDeclaredErrorDecoder.DeclaredErrors(${errorVal(error)}.fromErrorValue(_))"
        case None => "_root_.golem.tool.ToolDeclaredErrorDecoder.NoDeclaredErrors"
      }

    private def decode(leaf: FlattenedLeaf): String =
      leaf.codec.projectedOkType match {
        case Some(ok) =>
          s"__result => _root_.golem.tool.ToolCallPreparation.decodeValue(__result, _root_.scala.Predef.implicitly[_root_.golem.schema.FromSchema[$ok]], _root_.scala.Predef.implicitly[_root_.golem.schema.IntoSchema[$ok]].graph)"
        case None => "_root_.golem.tool.ToolCallPreparation.decodeUnit"
      }

    private def prepared(leaf: FlattenedLeaf): String =
      s"""_root_.golem.tool.PreparedToolCall(
      commandPath = ${ToolProjectionRendering.stringList(leaf.commandPath)},
      input = _root_.golem.tool.ToolCallPreparation.prepareInput(
        __descriptor,
        ${ToolProjectionRendering.stringList(leaf.commandPath)},
        inheritedPrefix,
        params
      ),
      stdin = stdin,
      errors = ${errors(leaf)},
      decodeValue = ${decode(leaf)}
    )"""

    private def awaitMethod(leaf: FlattenedLeaf): String = {
      val error = errorType(leaf)
      val value = valueType(leaf)
      s"""  def __await_${leaf.name}[B <: _root_.golem.tool.ToolCallBackend](
    backend: B,
    inheritedPrefix: _root_.scala.List[_root_.golem.tool.CanonicalInputValue],
    params: _root_.scala.Either[_root_.golem.tool.ToolInvokeError[_root_.scala.Nothing], _root_.scala.List[(_root_.scala.Predef.String, _root_.golem.schema.SchemaValue)]],
    stdin: _root_.scala.Option[backend.Stdin]
  ): backend.Awaited[$error, $value] =
    backend.awaitNoStdout(${prepared(leaf)})"""
    }

    private def startMethod(leaf: FlattenedLeaf): String = {
      val error               = errorType(leaf)
      val value               = valueType(leaf)
      val (family, operation) =
        (leaf.codec.projectedOkType, leaf.codec.hasStdout) match {
          case (_, false)      => (s"backend.StartedNoStdout[$error, $value]", "startNoStdout")
          case (None, true)    => (s"backend.StartedStdoutOnly[$error]", "startStdoutOnly")
          case (Some(_), true) => (s"backend.StartedValueStdout[$error, $value]", "startValueStdout")
        }
      s"""  def __start_${leaf.name}[B <: _root_.golem.tool.ToolCallBackend](
    backend: B,
    inheritedPrefix: _root_.scala.List[_root_.golem.tool.CanonicalInputValue],
    params: _root_.scala.Either[_root_.golem.tool.ToolInvokeError[_root_.scala.Nothing], _root_.scala.List[(_root_.scala.Predef.String, _root_.golem.schema.SchemaValue)]],
    stdin: _root_.scala.Option[backend.Stdin]
  ): $family =
    backend.$operation(${prepared(leaf)})"""
    }

    def render(): String = {
      val methods = tool.flattenedLeaves.flatMap(leaf => List(awaitMethod(leaf), startMethod(leaf)))
      val sb      = new StringBuilder
      if (tool.pkg.nonEmpty) sb.append(s"package ${tool.pkg}\n\n")
      requiredImports.foreach(importStatement => sb.append(importStatement).append("\n"))
      if (requiredImports.nonEmpty) sb.append("\n")
      sb.append("/** @internal Generated shared tool call projection. Do not edit. */\n")
      if (tool.pkg.nonEmpty) sb.append(s"private[${tool.pkg.split('.').last}] ")
      sb.append(s"object $projectionName {\n")
      val toolType = if (tool.pkg.isEmpty) tool.name else s"_root_.${tool.pkg}.${tool.name}"
      sb.append(
        "  private lazy val __descriptor: _root_.scala.Either[_root_.golem.tool.ToolBuildError, _root_.golem.tool.ExtendedToolType] =\n"
      )
      sb.append(s"    _root_.golem.runtime.macros.ToolDefinitionMacro.tryMetadata[$toolType]\n\n")
      sb.append(
        "  def __prefixInputModel(commandPath: _root_.scala.List[_root_.scala.Predef.String]): _root_.scala.Either[_root_.scala.Predef.String, _root_.golem.tool.CanonicalInputModel] =\n"
      )
      sb.append("    _root_.golem.tool.ToolClientRuntime.prefixInputModel(__descriptor, commandPath)\n\n")
      sb.append(
        "  def __underlyingBackend(underlying: _root_.golem.tool.RawToolUnderlying): _root_.golem.tool.UnderlyingToolCallBackend =\n"
      )
      sb.append("    new _root_.golem.tool.UnderlyingToolCallBackend(underlying, __descriptor)\n\n")
      errorVals.foreach { case (errorType, name) =>
        sb.append(s"  private lazy val $name: _root_.golem.tool.ToolErrorSchema[$errorType] =\n")
        sb.append(s"    _root_.golem.runtime.macros.ToolErrorSchemaDerivation.derive[$errorType]\n\n")
      }
      sb.append(methods.mkString("\n\n"))
      sb.append("\n}\n")
      sb.toString
    }
  }
}
