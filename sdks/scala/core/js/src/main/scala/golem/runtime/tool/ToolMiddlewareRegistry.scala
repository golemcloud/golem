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
  ToolMiddlewareDescriptor,
  UniversalToolMiddlewareHandle
}

import scala.collection.mutable

private[golem] object ToolMiddlewareRegistry {

  sealed trait ToolMiddlewareInvoker extends Product with Serializable
  object ToolMiddlewareInvoker {
    final case class Monomorphic(
      presented: ExtendedToolType,
      expected: ExtendedToolType,
      handle: MonomorphicToolMiddlewareHandle
    ) extends ToolMiddlewareInvoker

    final case class Universal(
      handle: UniversalToolMiddlewareHandle
    ) extends ToolMiddlewareInvoker
  }

  private final case class Entry(
    descriptor: ToolMiddlewareDescriptor,
    invoker: ToolMiddlewareInvoker
  )

  private val entries: mutable.LinkedHashMap[String, Entry] = mutable.LinkedHashMap.empty

  def register(
    descriptor: ToolMiddlewareDescriptor,
    invoker: ToolMiddlewareInvoker
  ): Unit = {
    if (entries.contains(descriptor.name))
      throw new IllegalArgumentException(
        s"duplicate tool middleware registration for middleware name: ${descriptor.name}"
      )
    entries.update(descriptor.name, Entry(descriptor, invoker))
  }

  def allMiddlewares: List[ToolMiddlewareDescriptor] =
    entries.toList.sortBy(_._1).map(_._2.descriptor)

  def getMiddleware(name: String): Option[ToolMiddlewareDescriptor] =
    entries.get(name).map(_.descriptor)

  def getInvoker(name: String): Option[ToolMiddlewareInvoker] =
    entries.get(name).map(_.invoker)

  private[golem] def clearForTests(): Unit =
    entries.clear()
}
