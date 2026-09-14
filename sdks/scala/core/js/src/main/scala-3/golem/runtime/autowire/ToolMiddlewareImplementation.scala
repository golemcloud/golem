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

package golem.runtime.autowire

import golem.runtime.macros.ToolMiddlewareMacro
import golem.runtime.tool.ToolMiddlewareImplementationRuntime
import golem.tool.{
  MonomorphicToolMiddlewareHandle,
  RawToolUnderlying,
  UniversalToolMiddleware,
  UniversalToolMiddlewareHandle
}

object ToolMiddlewareImplementation {

  private[golem] def registerHandle(handle: MonomorphicToolMiddlewareHandle): Unit =
    ToolMiddlewareImplementationRuntime.registerMonomorphic(handle)

  private[golem] def registerUniversalHandle(handle: UniversalToolMiddlewareHandle): Unit =
    ToolMiddlewareImplementationRuntime.registerUniversal(handle)

  inline def registerTransparent[Presented, Underlying, Surface, Impl <: Surface](
    underlying: RawToolUnderlying => Underlying
  ): Unit =
    registerHandle(
      ToolMiddlewareMacro.transparentHandle[Presented, Underlying, Surface, Impl](underlying)
    )

  inline def registerAdapter[Presented, Expected, Underlying, Surface, Impl <: Surface](
    underlying: RawToolUnderlying => Underlying
  ): Unit =
    registerHandle(
      ToolMiddlewareMacro.adapterHandle[Presented, Expected, Underlying, Surface, Impl](underlying)
    )

  inline def registerUniversal[Impl <: UniversalToolMiddleware]: Unit =
    registerUniversalHandle(ToolMiddlewareMacro.universalHandle[Impl])
}
