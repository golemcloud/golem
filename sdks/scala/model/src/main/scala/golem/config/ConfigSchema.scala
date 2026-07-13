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

package golem.config

import zio.blocks.schema.Schema

trait ConfigSchema[T] {
  def describe(path: List[String]): List[AgentConfigDeclaration]
}

object ConfigSchema {
  def apply[T](implicit cs: ConfigSchema[T]): ConfigSchema[T] = cs

  implicit def fromSchema[A](implicit schema: Schema[A]): ConfigSchema[A] =
    new ConfigSchema[A] {
      def describe(path: List[String]): List[AgentConfigDeclaration] =
        ConfigIntrospection.declarations[A](path)
    }
}
