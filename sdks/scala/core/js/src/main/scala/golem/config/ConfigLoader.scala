/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.config

import golem.runtime.autowire.SchemaPayload
import golem.runtime.rpc.host.AgentHostApi
import golem.schema.{FromSchema, IntoSchema}

private[golem] object ConfigLoader extends ConfigFieldLoader {

  override def loadLocal[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): A =
    loadValue[A](path)

  override def loadSecret[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): Secret[A] =
    new Secret[A](path, () => loadValue[A](path))

  def loadConfig[T](builder: ConfigBuilder[T]): Config[T] =
    Config.eager(builder.build(Nil, this))

  def createLazyConfig[T](builder: ConfigBuilder[T]): Config[T] =
    Config(() => builder.build(Nil, ConfigLoader))

  /**
   * Fetches the host config value at `path` as a `schema-value-tree`, passing
   * the expected schema graph (derived from `IntoSchema[A]`) as a migration
   * hint, then decodes it via `FromSchema[A]`.
   */
  def loadValue[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): A = {
    val expectedGraph = SchemaPayload.graph[A]
    val tree          = AgentHostApi.getConfigValue(path, expectedGraph)
    SchemaPayload.decode[A](tree) match {
      case Right(a)  => a
      case Left(err) =>
        throw new RuntimeException(s"Failed to decode config value at path ${path.mkString(".")}: $err")
    }
  }
}
