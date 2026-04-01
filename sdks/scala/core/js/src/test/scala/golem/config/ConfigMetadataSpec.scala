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

import golem.BaseAgent
import golem.data.{DataType, ElementSchema}
import golem.host.js.{JsAgentConfigDeclaration, JsAgentConfigSource}
import golem.runtime.annotations.agentDefinition
import golem.runtime.autowire.WitTypeBuilder
import golem.runtime.macros.AgentMacros
import golem.runtime.AgentMetadata
import zio.test._

import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js

object ConfigMetadataSpec extends ZIOSpecDefault {

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
      test("local field has correct value type") {
        assertTrue(
          configMeta.config.exists(d =>
            d.path == List("host") && d.valueType == ElementSchema.Component(DataType.StringType)
          )
        )
      },
      test("secret field has correct value type") {
        assertTrue(
          configMeta.config.exists(d =>
            d.path == List("secret") && d.valueType == ElementSchema.Component(DataType.StringType)
          )
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
    ),
    suite("JsAgentConfigDeclaration encoding")(
      test("local declaration encodes to JS correctly") {
        val decl =
          AgentConfigDeclaration(AgentConfigSource.Local, List("host"), ElementSchema.Component(DataType.StringType))
        val jsDecl = encodeDeclaration(decl)
        assertTrue(
          jsDecl.source == ("local": JsAgentConfigSource),
          jsDecl.path.length == 1,
          jsDecl.path(0) == "host"
        )
      },
      test("secret declaration encodes to JS correctly") {
        val decl =
          AgentConfigDeclaration(AgentConfigSource.Secret, List("apiKey"), ElementSchema.Component(DataType.StringType))
        val jsDecl = encodeDeclaration(decl)
        assertTrue(
          jsDecl.source == ("secret": JsAgentConfigSource),
          jsDecl.path.length == 1,
          jsDecl.path(0) == "apiKey"
        )
      },
      test("multi-segment path encodes correctly") {
        val decl = AgentConfigDeclaration(
          AgentConfigSource.Local,
          List("db", "host"),
          ElementSchema.Component(DataType.StringType)
        )
        val jsDecl = encodeDeclaration(decl)
        assertTrue(
          jsDecl.path.length == 2,
          jsDecl.path(0) == "db",
          jsDecl.path(1) == "host"
        )
      },
      test("empty declarations produce empty JS array") {
        val arr = encodeDeclarations(Nil)
        assertTrue(arr.length == 0)
      },
      test("multiple declarations encode to correct-length JS array") {
        val decls = List(
          AgentConfigDeclaration(AgentConfigSource.Local, List("a"), ElementSchema.Component(DataType.StringType)),
          AgentConfigDeclaration(AgentConfigSource.Secret, List("b"), ElementSchema.Component(DataType.IntType)),
          AgentConfigDeclaration(AgentConfigSource.Local, List("c", "d"), ElementSchema.Component(DataType.BoolType))
        )
        val arr = encodeDeclarations(decls)
        assertTrue(arr.length == 3)
      }
    )
  )

  // ---------------------------------------------------------------------------
  // Helpers — reimplement the encoding logic to test it without AgentDefinition
  // ---------------------------------------------------------------------------

  private def encodeDeclaration(decl: AgentConfigDeclaration): JsAgentConfigDeclaration = {
    val source: JsAgentConfigSource = decl.source match {
      case AgentConfigSource.Local  => "local"
      case AgentConfigSource.Secret => "secret"
    }
    val path    = js.Array(decl.path: _*)
    val witType = decl.valueType match {
      case ElementSchema.Component(dataType) => WitTypeBuilder.build(dataType)
      case _                                 => throw new UnsupportedOperationException("Only component schemas supported")
    }
    JsAgentConfigDeclaration(source, path, witType)
  }

  private def encodeDeclarations(decls: List[AgentConfigDeclaration]): js.Array[JsAgentConfigDeclaration] = {
    val arr = new js.Array[JsAgentConfigDeclaration]()
    decls.foreach(d => arr.push(encodeDeclaration(d)))
    arr
  }
}
