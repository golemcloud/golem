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

import golem.Principal
import golem.host.js.tool.{JsWasiInputStream, JsWasiOutputStream}
import golem.schema.wire.WitTypedSchemaValue
import golem.tool.ExtendedToolType
import golem.tool.wire.{WitTool, WitToolError}

import scala.collection.mutable
import scala.concurrent.Future

/**
 * The value a tool invoker produces on success: the optional structured result
 * (a self-contained `typed-schema-value`) and the optional stdout stream
 * handle, mirroring `golem:tool/common@0.1.0`'s `invocation-result`.
 */
final case class ToolInvocationResult(
  result: Option[WitTypedSchemaValue],
  stdout: Option[JsWasiOutputStream]
)

/**
 * Name-keyed registry of the tools this component exposes through the
 * `golem:tool/guest@0.1.0` exports.
 *
 * The tool-implementation macro registers each implemented tool together with
 * its invoker at module initialization time (the same pattern as
 * [[golem.runtime.autowire.AgentRegistry]]); definition-only registrations (no
 * invoker) are discoverable but not invocable.
 */
private[golem] object ToolRegistry {

  /**
   * Dispatches one `guest.invoke` call: `(command-path, input, stdin,
   * principal)` to either a successful [[ToolInvocationResult]] or a
   * [[WitToolError]].
   */
  type ToolInvoker =
    (
      List[String],
      WitTypedSchemaValue,
      Option[JsWasiInputStream],
      Principal
    ) => Future[Either[WitToolError, ToolInvocationResult]]

  private final case class Entry(
    extended: ExtendedToolType,
    encoded: WitTool,
    invoker: Option[ToolInvoker]
  )

  private val entries: mutable.LinkedHashMap[String, Entry] = mutable.LinkedHashMap.empty

  /**
   * Registers a tool definition without an invoker (discoverable via
   * `discover-tools` / `get-tool`, but not invocable).
   *
   * @throws IllegalArgumentException
   *   if the descriptor fails validation or a tool with the same name is
   *   already registered
   */
  def register(tool: ExtendedToolType): Unit =
    registerInner(tool, None)

  /**
   * Registers a tool definition together with its invoker.
   *
   * @throws IllegalArgumentException
   *   if the descriptor fails validation or a tool with the same name is
   *   already registered
   */
  def registerInvoker(tool: ExtendedToolType, invoker: ToolInvoker): Unit =
    registerInner(tool, Some(invoker))

  private def registerInner(tool: ExtendedToolType, invoker: Option[ToolInvoker]): Unit =
    synchronized {
      val encoded = tool.tryToTool match {
        case Right(t)    => t
        case Left(error) => throw new IllegalArgumentException(s"tool descriptor build failed: $error")
      }
      val name = tool.toolName
      if (entries.contains(name)) {
        throw new IllegalArgumentException(s"duplicate tool registration for tool name: $name")
      }
      entries.update(name, Entry(tool, encoded, invoker))
    }

  /** All registered tools' wire descriptors, sorted by tool name. */
  def allTools: List[WitTool] =
    synchronized {
      entries.toList.sortBy(_._1).map(_._2.encoded)
    }

  def getTool(name: String): Option[WitTool] =
    synchronized {
      entries.get(name).map(_.encoded)
    }

  def getExtendedTool(name: String): Option[ExtendedToolType] =
    synchronized {
      entries.get(name).map(_.extended)
    }

  def getInvoker(name: String): Option[ToolInvoker] =
    synchronized {
      entries.get(name).flatMap(_.invoker)
    }

  private[golem] def clearForTests(): Unit =
    synchronized {
      entries.clear()
    }
}
