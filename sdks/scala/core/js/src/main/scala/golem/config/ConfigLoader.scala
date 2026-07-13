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

import golem.host.SecretApi
import golem.runtime.autowire.SchemaPayload
import golem.runtime.rpc.host.AgentHostApi
import golem.schema.{FromSchema, IntoSchema, SchemaGraph, SchemaType, SchemaTypeBody, SchemaValue, SecretSpec}
import golem.schema.wire.SchemaWire
import golem.host.SchemaWireInterop

private[golem] object ConfigLoader extends ConfigFieldLoader {

  override def loadLocal[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): A =
    loadValue[A](path)

  override def loadSecret[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): Secret[A] =
    new Secret[A](path, () => loadSecretValue[A](path))

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

  private def loadSecretValue[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): A = {
    val expectedHandleGraph = SchemaPayload.graphFromModel(secretGraph(into.graph))
    val handleTree          = AgentHostApi.getConfigValue(path, expectedHandleGraph)
    val handleValue         = SchemaWire.schemaValueFromWit(SchemaWireInterop.valueTreeFromJs(handleTree))
    val handle              = handleValue match {
      case SchemaValue.SecretValue(h) => h
      case other                      =>
        throw new RuntimeException(s"Expected secret handle at path ${path.mkString(".")}, got $other")
    }

    val innerGraph = SchemaPayload.graph[A]
    val revealed   = SecretApi.reveal(handle, innerGraph)
    if (handle.take().isEmpty)
      throw new RuntimeException(s"Secret handle at path ${path.mkString(".")} was already transferred")
    SchemaPayload.decode[A](revealed) match {
      case Right(a)  => a
      case Left(err) =>
        throw new RuntimeException(s"Failed to decode revealed secret at path ${path.mkString(".")}: $err")
    }
  }

  private def secretGraph(inner: SchemaGraph): SchemaGraph =
    SchemaGraph(inner.defs, SchemaType(SchemaTypeBody.SecretType(SecretSpec(inner.root))))
}
