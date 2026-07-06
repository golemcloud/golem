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

import golem.runtime.macros.ToolImplementationMacro
import golem.runtime.tool.ToolImplementationRuntime

object ToolImplementation {

  /**
   * Registers a tool implementation class for a `@toolDefinition` trait: the
   * macro derives the tool descriptor and the invocation surface from the
   * trait, and the implementation is registered into the tool registry so it is
   * discoverable and invocable through the `golem:tool/guest` exports.
   */
  inline def registerClass[Trait, Impl <: Trait]: Unit =
    ToolImplementationRuntime.register(ToolImplementationMacro.handle[Trait, Impl])
}
