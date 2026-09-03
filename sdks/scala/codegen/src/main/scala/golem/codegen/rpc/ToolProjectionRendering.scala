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

import golem.codegen.rpc.ToolProjectionIR.{LeafReturn, Param, ProjectedParam}

/**
 * Shared leaf projection rendering parameterized by transport and error policy.
 */
object ToolProjectionRendering {

  sealed trait Policy {
    def runtime: String
    def errorType: String
    def useProjectedTypes: Boolean
  }

  case object AmbientClient extends Policy {
    val runtime: String            = "_root_.golem.tool.ToolClientRuntime"
    val errorType: String          = "_root_.golem.tool.ToolError"
    val useProjectedTypes: Boolean = false
  }

  case object InvocationUnderlying extends Policy {
    val runtime: String            = "_root_.golem.tool.ToolUnderlyingRuntime"
    val errorType: String          = "_root_.golem.tool.ToolInvokeError"
    val useProjectedTypes: Boolean = true
  }

  def paramType(param: Param, policy: Policy): String =
    if (param.isPrincipal) "_root_.golem.Principal"
    else if (param.isStdin && policy == InvocationUnderlying)
      "_root_.golem.tool.ToolMiddlewareInputHandle"
    else if (param.isStdin) "_root_.golem.tool.ToolInputStream"
    else if (param.isStdout && policy == InvocationUnderlying)
      "_root_.golem.tool.ToolMiddlewareOutputHandle"
    else if (param.isStdout) "_root_.golem.tool.ToolOutputStream"
    else if (policy.useProjectedTypes) param.projectedTypeExpr
    else param.typeExpr

  def paramDecl(projected: ProjectedParam, policy: Policy): String = {
    val metadata =
      if (
        policy == InvocationUnderlying &&
        !projected.param.isPrincipal &&
        !projected.param.isStdin &&
        !projected.param.isStdout
      ) {
        val countFlag = ToolProjectionIR.isCountFlag(projected.param)
        s"""@_root_.golem.runtime.annotations.internalToolMiddlewareField("${projected.canonicalName}", $countFlag) """
      } else ""
    s"$metadata${projected.param.ident}: ${paramType(projected.param, policy)}"
  }

  def successType(codec: LeafReturn, policy: Policy): String = {
    val okType     = if (policy.useProjectedTypes) codec.projectedOkType else codec.okType
    val stdoutType =
      if (policy == InvocationUnderlying) "_root_.golem.tool.ToolMiddlewareOutputHandle"
      else "_root_.golem.tool.ToolOutputStream"
    (okType, codec.hasStdout) match {
      case (Some(ok), true)  => s"($ok, $stdoutType)"
      case (None, true)      => stdoutType
      case (Some(ok), false) => ok
      case (None, false)     => "_root_.scala.Unit"
    }
  }

  def returnType(codec: LeafReturn, policy: Policy): String = {
    val error =
      if (policy.useProjectedTypes) codec.projectedErrType.getOrElse("_root_.scala.Nothing")
      else codec.errType.getOrElse("_root_.scala.Nothing")
    s"_root_.scala.concurrent.Future[_root_.scala.Either[${policy.errorType}[$error], ${successType(codec, policy)}]]"
  }

  def valueEntry(projected: ProjectedParam, policy: Policy): String = {
    val param = projected.param
    if (ToolProjectionIR.isCountFlag(param))
      s"""("${projected.canonicalName}", ${policy.runtime}.countFlagValue(${param.ident}))"""
    else
      s"""("${projected.canonicalName}", _root_.scala.Predef.implicitly[_root_.golem.schema.IntoSchema[${paramType(
          param,
          policy
        )}]].toValue(${param.ident}))"""
  }

  def runExpression(
    policy: Policy,
    codec: LeafReturn,
    transport: String,
    commandPath: String,
    input: String,
    stdin: String,
    errorSchema: Option[String],
    descriptor: Option[String] = None
  ): String = {
    val errorType = if (policy.useProjectedTypes) codec.projectedErrType else codec.errType
    val arguments = descriptor.fold(s"$transport, $commandPath, $input, $stdin") { value =>
      s"$transport, $value, $commandPath, $input, $stdin"
    }
    errorType match {
      case Some(error) =>
        s"${policy.runtime}.run[$error]($arguments, ${errorSchema.get}.fromErrorPayloadValue(_))"
      case None =>
        s"${policy.runtime}.runInfallible($arguments)"
    }
  }

  def decodeExpression(
    policy: Policy,
    codec: LeafReturn,
    result: String
  ): String = {
    val okType = if (policy.useProjectedTypes) codec.projectedOkType else codec.okType
    (okType, codec.hasStdout) match {
      case (Some(ok), true) =>
        s"${policy.runtime}.decodeValueStdoutResult($result, _root_.scala.Predef.implicitly[_root_.golem.schema.FromSchema[$ok]])"
      case (None, true) =>
        s"${policy.runtime}.decodeStdoutResult($result)"
      case (Some(ok), false) =>
        s"${policy.runtime}.decodeValueResult($result, _root_.scala.Predef.implicitly[_root_.golem.schema.FromSchema[$ok]])"
      case (None, false) =>
        s"${policy.runtime}.decodeUnitResult($result)"
    }
  }

  def stringList(entries: List[String]): String =
    if (entries.isEmpty) "_root_.scala.Nil"
    else entries.map(entry => s""""$entry"""").mkString("_root_.scala.List(", ", ", ")")

  def commandPath(base: Option[String], localPath: List[String]): String =
    base match {
      case None       => stringList(localPath)
      case Some(expr) => localPath.foldLeft(expr)((path, segment) => s"""$path :+ "$segment"""")
    }
}
