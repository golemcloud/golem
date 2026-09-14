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

package golem.runtime.tool

import golem.tool.{
  ExtendedToolType,
  MonomorphicToolMiddlewareHandle,
  ToolBuildCtx,
  ToolMiddlewareDescriptor,
  ToolMiddlewareScope,
  UniversalToolMiddlewareHandle
}

private[golem] object ToolMiddlewareImplementationRuntime {

  def registerMonomorphic(handle: MonomorphicToolMiddlewareHandle): Unit = {
    val descriptor = handle.descriptor(new ToolBuildCtx) match {
      case Right(value) => value
      case Left(error)  =>
        throw new IllegalArgumentException(s"tool middleware descriptor build failed: ${error.message}")
    }
    val presented = handle.presented(new ToolBuildCtx) match {
      case Right(value) => value
      case Left(error)  =>
        throw new IllegalArgumentException(s"presented tool descriptor build failed: ${error.message}")
    }
    val expected = handle.expected(new ToolBuildCtx) match {
      case Right(value) => value
      case Left(error)  =>
        throw new IllegalArgumentException(s"expected tool descriptor build failed: ${error.message}")
    }
    val presentedWire = encodeTool("presented", presented)
    val expectedWire  = encodeTool("expected", expected)
    validateName(descriptor)
    descriptor.scope match {
      case ToolMiddlewareScope.Monomorphic(`presentedWire`, Some(`expectedWire`)) => ()
      case ToolMiddlewareScope.Monomorphic(_, None)                               =>
        throw new IllegalArgumentException("monomorphic tool middleware requires an expected descriptor")
      case ToolMiddlewareScope.Monomorphic(_, _) =>
        throw new IllegalArgumentException(
          "tool middleware descriptor scope does not match its presented and expected tool descriptors"
        )
      case ToolMiddlewareScope.Universal =>
        throw new IllegalArgumentException("monomorphic tool middleware must use a monomorphic descriptor scope")
    }
    ToolMiddlewareRegistry.register(
      descriptor,
      ToolMiddlewareRegistry.ToolMiddlewareInvoker.Monomorphic(presented, expected, handle)
    )
  }

  def registerUniversal(handle: UniversalToolMiddlewareHandle): Unit = {
    validateName(handle.descriptor)
    if (handle.descriptor.scope != ToolMiddlewareScope.Universal)
      throw new IllegalArgumentException("universal tool middleware must use a universal descriptor scope")
    ToolMiddlewareRegistry.register(
      handle.descriptor,
      ToolMiddlewareRegistry.ToolMiddlewareInvoker.Universal(handle)
    )
  }

  private def validateName(descriptor: ToolMiddlewareDescriptor): Unit =
    if (descriptor.name.trim.isEmpty)
      throw new IllegalArgumentException("tool middleware descriptor name must not be empty")

  private def encodeTool(label: String, tool: ExtendedToolType) =
    tool.tryToTool match {
      case Right(value) => value
      case Left(error)  =>
        throw new IllegalArgumentException(s"tool middleware $label descriptor build failed: ${error.message}")
    }
}
