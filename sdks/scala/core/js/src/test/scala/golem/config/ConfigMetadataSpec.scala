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

import golem.BaseAgent
import golem.runtime.annotations.agentDefinition
import golem.runtime.macros.AgentMacros
import golem.runtime.AgentMetadata
import golem.schema.{IntoSchema, SchemaGraph, SchemaType, SchemaTypeBody, SecretSpec}
import zio.test._

import zio.blocks.schema.Schema

import scala.concurrent.Future

object ConfigMetadataSpec extends ZIOSpecDefault {

  private def secretGraph[A](implicit into: IntoSchema[A]): SchemaGraph =
    SchemaGraph(into.graph.defs, SchemaType(SchemaTypeBody.SecretType(SecretSpec(into.graph.root))))

  // ---------------------------------------------------------------------------
  // Config types — Schema instances provide ConfigSchema automatically
  // ---------------------------------------------------------------------------

  final case class TestDbConfig(host: String, secret: Secret[String])
  object TestDbConfig {
    implicit val schema: Schema[TestDbConfig] = Schema.derived
  }

  final case class NestedAppConfig(appName: String, db: TestDbConfig)
  object NestedAppConfig {
    implicit val schema: Schema[NestedAppConfig] = Schema.derived
  }

  // ---------------------------------------------------------------------------
  // Agent traits (metadata-only, no registration to avoid WASM host deps)
  // ---------------------------------------------------------------------------

  @agentDefinition()
  trait ConfigTestAgent extends BaseAgent with AgentConfig[TestDbConfig] {
    class Id()
    def ping(): Future[String]
  }

  @agentDefinition()
  trait NestedConfigAgent extends BaseAgent with AgentConfig[NestedAppConfig] {
    class Id()
    def info(): Future[String]
  }

  @agentDefinition()
  trait NoConfigAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  // Generate metadata via macro without full agent registration
  private lazy val configMeta: AgentMetadata   = AgentMacros.agentMetadata[ConfigTestAgent]
  private lazy val nestedMeta: AgentMetadata   = AgentMacros.agentMetadata[NestedConfigAgent]
  private lazy val noConfigMeta: AgentMetadata = AgentMacros.agentMetadata[NoConfigAgent]

  // ---------------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------------

  override def spec = suite("ConfigMetadataSpec")(
    suite("agent with config")(
      test("metadata has non-empty config declarations") {
        assertTrue(configMeta.config.nonEmpty)
      },
      test("metadata has correct number of config declarations") {
        assertTrue(configMeta.config.size == 2)
      },
      test("local field appears with Local source") {
        assertTrue(
          configMeta.config.exists(d => d.path == List("host") && d.source == AgentConfigSource.Local)
        )
      },
      test("secret field appears with Secret source") {
        assertTrue(
          configMeta.config.exists(d => d.path == List("secret") && d.source == AgentConfigSource.Secret)
        )
      },
      test("local field has schema-native value type") {
        assertTrue(
          configMeta.config.exists(d => d.path == List("host") && d.valueType == IntoSchema[String].graph)
        )
      },
      test("secret field has schema-native secret handle value type") {
        assertTrue(
          configMeta.config.exists(d => d.path == List("secret") && d.valueType == secretGraph[String])
        )
      }
    ),
    suite("agent with nested config")(
      test("metadata has declarations for all leaf fields") {
        assertTrue(nestedMeta.config.size == 3)
      },
      test("top-level field has single-segment path") {
        assertTrue(
          nestedMeta.config.exists(d => d.path == List("appName") && d.source == AgentConfigSource.Local)
        )
      },
      test("nested local field has multi-segment path") {
        assertTrue(
          nestedMeta.config.exists(d => d.path == List("db", "host") && d.source == AgentConfigSource.Local)
        )
      },
      test("nested secret field has multi-segment path") {
        assertTrue(
          nestedMeta.config.exists(d => d.path == List("db", "secret") && d.source == AgentConfigSource.Secret)
        )
      }
    ),
    suite("agent without config")(
      test("metadata has empty config declarations") {
        assertTrue(noConfigMeta.config.isEmpty)
      }
    )
  )
}
