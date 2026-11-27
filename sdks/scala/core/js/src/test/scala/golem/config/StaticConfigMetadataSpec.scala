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

import golem.runtime.annotations.agentDefinition
import golem.runtime.macros.{AgentDefinitionMacro, AgentImplementationMacro}
import golem.schema.*
import golem.schema.wire.*
import zio.blocks.schema.{Modifier, Schema}
import zio.test.*
import scala.concurrent.Future

object StaticConfigMetadataSpec extends ZIOSpecDefault {
  final case class Settings(label: String, count: golem.UInt) derives Schema
  final case class Node(display_name: String, next: Option[Node]) derives Schema
  final case class Values(
    @Modifier.config("golem.config.default", "{\"label\":\"code-default\",\"count\":31}") settings: Settings,
    @Modifier.config("golem.config.default", "[null,7,\"retained\"]") slots: (Option[Int], Option[Int], String),
    @Modifier.config("golem.config.default", "null") cleared: Option[String],
    omitted: Option[String],
    recursive_tree: Node
  ) derives Schema

  @agentDefinition()
  trait StaticAgent extends AgentConfig[Values] {
    class Id()
    def read(): Future[String]
  }

  final class StaticImpl(config: Config[Values]) extends StaticAgent {
    def read(): Future[String] = Future.successful(config.settings.label)
  }

  final case class BadInteger(@Modifier.config("golem.config.default", "\"bad\"") value: BigInt) derives Schema
  final case class BadDecimals(@Modifier.config("golem.config.default", "[\"1.25\",\"bad\"]") values: List[BigDecimal])
      derives Schema
  final case class NativeNumbers(
    @Modifier.config("golem.config.default", "\"123456789012345678901234567890\"") integer: BigInt,
    @Modifier.config("golem.config.default", "[\"1.25\",\"-0.5\"]") decimals: List[BigDecimal]
  ) derives Schema
  @agentDefinition() trait BadIntegerAgent    extends AgentConfig[BadInteger]    { class Id() }
  @agentDefinition() trait BadDecimalsAgent   extends AgentConfig[BadDecimals]   { class Id() }
  @agentDefinition() trait NativeNumbersAgent extends AgentConfig[NativeNumbers] { class Id() }

  private val model = AgentDefinitionMacro.generate[StaticAgent].config
  private val wire  = AgentDefinitionMacro.generateWire[StaticAgent].reflected.config

  override def spec = suite("StaticConfigMetadataSpec")(
    test("compiled config builder decodes whole static captures without consulting defaults") {
      val supplied =
        Values(Settings("creation", golem.UInt(0)), (Some(-4), None, ""), None, Some("set"), Node("head", None))
      val loader = new ConfigFieldLoader {
        def loadLocal[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): A =
          throw new AssertionError("Compiled builder must use wire captures")
        def loadSecret[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): Secret[A] =
          throw new AssertionError("Static data must not read secrets")
        def loadLocalWire[A](path: List[String], graph: WitSchemaGraph, codec: ConcreteCodec[A]): A = {
          val encoded = path match {
            case List("settings")      => ConcreteCodec.derived[Settings].encodeValue(supplied.settings)
            case List("slots")         => ConcreteCodec.derived[(Option[Int], Option[Int], String)].encodeValue(supplied.slots)
            case List("cleared")       => ConcreteCodec.derived[Option[String]].encodeValue(supplied.cleared)
            case List("omitted")       => ConcreteCodec.derived[Option[String]].encodeValue(supplied.omitted)
            case List("recursiveTree") => ConcreteCodec.derived[Node].encodeValue(supplied.recursive_tree)
            case other                 => throw new AssertionError(other.mkString("."))
          }
          codec.decode(encoded)
        }
        def loadSecretWire[A](
          path: List[String],
          graph: WitSchemaGraph,
          handleGraph: WitSchemaGraph,
          codec: ConcreteCodec[A]
        ): Secret[A] =
          throw new AssertionError("Static data must not read secrets")
      }
      val implementation = AgentImplementationMacro.wireImplementationTypeFromClass[StaticAgent, StaticImpl]
      assertTrue(implementation.configBuilder.get.asInstanceOf[ConfigBuilder[Values]].build(Nil, loader) == supplied)
    },
    test("compiler metadata preserves static shapes and canonical defaults like reflection") {
      assertTrue(
        wire.map(d => d.path -> d.defaultValue) == model.map(d => d.path -> d.defaultValue),
        wire.map(_.defaultValue) == List(
          Some(SchemaValue.RecordValue(List(SchemaValue.StringValue("code-default"), SchemaValue.U32Value(31)))),
          Some(
            SchemaValue.TupleValue(
              List(
                SchemaValue.OptionValue(None),
                SchemaValue.OptionValue(Some(SchemaValue.S32Value(7))),
                SchemaValue.StringValue("retained")
              )
            )
          ),
          Some(SchemaValue.OptionValue(None)),
          None,
          None
        ),
        wire.map(_.path) == List("settings", "slots", "cleared", "omitted", "recursiveTree").map(List(_)),
        wire.forall { d =>
          val other  = model.find(_.path == d.path).get.valueType
          val actual = SchemaFingerprintV1.compute(d.valueType, Some(d.valueType.root))
          actual.isRight && actual == SchemaFingerprintV1.compute(other, Some(other.root))
        }
      )
    },
    test("compiled and reflective metadata reject malformed native numbers before export") {
      val compiledInteger  = scala.util.Try(AgentDefinitionMacro.generateWire[BadIntegerAgent])
      val compiledDecimals = scala.util.Try(AgentDefinitionMacro.generateWire[BadDecimalsAgent])
      val modelInteger     = scala.util.Try(AgentDefinitionMacro.generate[BadIntegerAgent])
      val modelDecimals    = scala.util.Try(AgentDefinitionMacro.generate[BadDecimalsAgent])
      val valid            = AgentDefinitionMacro.generateWire[NativeNumbersAgent].reflected.config
      assertTrue(
        List(compiledInteger, compiledDecimals, modelInteger, modelDecimals).forall(
          _.failed.get.getMessage.contains("Invalid config default")
        ),
        valid.map(_.defaultValue) == List(
          Some(SchemaValue.StringValue("123456789012345678901234567890")),
          Some(SchemaValue.ListValue(List(SchemaValue.StringValue("1.25"), SchemaValue.StringValue("-0.5"))))
        )
      )
    },
    test("invalid static defaults fail compilation rather than becoming read fallbacks") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.config.*
        import golem.runtime.annotations.agentDefinition
        import golem.runtime.macros.AgentDefinitionMacro
        import zio.blocks.schema.Modifier
        case class Bad(@Modifier.config("golem.config.default", "false") count: Int)
        @agentDefinition() trait BadAgent extends AgentConfig[Bad] { class Id() }
        AgentDefinitionMacro.generateWire[BadAgent]
      """)
      assertTrue(errors.exists(_.message.contains("Invalid config default")))
    }
  )
}
