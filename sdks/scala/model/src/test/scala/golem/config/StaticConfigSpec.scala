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

import golem.UInt
import golem.schema.*
import golem.schema.wire.*
import zio.blocks.schema.{Modifier, Schema}
import zio.blocks.schema.json.Json
import zio.test.*

object StaticConfigSpec extends ZIOSpecDefault {
  given Schema[(Option[Int], Option[Int], String)] = Schema.derived
  final case class Settings(label: String, count: UInt) derives Schema
  final case class Node(displayName: String, next: Option[Node]) derives Schema
  final case class Values(
    @Modifier.config("golem.config.default", "{\"label\":\"code-default\",\"count\":31}") settings: Settings,
    @Modifier.config("golem.config.default", "[null,7,\"retained\"]") slots: (Option[Int], Option[Int], String),
    @Modifier.config("golem.config.default", "null") cleared: Option[String],
    omitted: Option[String],
    recursive: Node,
    @Modifier.config("golem.config.default", "true") enabled: Boolean
  ) derives Schema

  final case class Bad(@Modifier.config("golem.config.default", "\"wrong\"") count: Int) derives Schema
  final case class Managed(@Modifier.config("golem.config.default", "\"private\"") key: Secret[String]) derives Schema
  final case class NativeNode(display_name: String, next: Option[NativeNode]) derives Schema
  final case class Native(
    @Modifier.config("golem.config.default", "19") max_chars: Int,
    @Modifier.config("golem.config.default", "{\"displayName\":\"head\",\"next\":null}") tree: NativeNode
  ) derives Schema
  final case class Collision(max_chars: Int, maxChars: Int) derives Schema
  final case class BadInteger(@Modifier.config("golem.config.default", "\"not-an-integer\"") value: BigInt) derives Schema
  final case class BadDecimals(@Modifier.config("golem.config.default", "[\"1.25\",\"bad\"]") values: List[BigDecimal]) derives Schema
  final case class NativeNumbers(
    @Modifier.config("golem.config.default", "\"123456789012345678901234567890\"") integer: BigInt,
    @Modifier.config("golem.config.default", "[\"1.25\",\"-0.5\"]") decimals: List[BigDecimal]
  ) derives Schema

  private val supplied = Values(
    Settings("creation", UInt(0L)),
    (Some(-4), None, ""),
    None,
    None,
    Node("head", Some(Node("tail", None))),
    false
  )

  override def spec = suite("StaticConfigSpec")(
    test("exports validated data defaults without flattening records or recursive schemas") {
      val declarations = ConfigSchema[Values].describe(Nil)
      val byPath       = declarations.map(d => d.path.mkString(".") -> d).toMap
      val settings     = byPath("settings")
      assertTrue(
        declarations.map(_.path) == List("settings", "slots", "cleared", "omitted", "recursive", "enabled").map(
          List(_)
        ),
        settings.defaultValue.contains(SchemaValue.RecordValue(List(SchemaValue.StringValue("code-default"), SchemaValue.U32Value(31)))),
        byPath("slots").defaultValue.contains(SchemaValue.TupleValue(List(SchemaValue.OptionValue(None), SchemaValue.OptionValue(Some(SchemaValue.S32Value(7))), SchemaValue.StringValue("retained")))),
        byPath("cleared").defaultValue.contains(SchemaValue.OptionValue(None)),
        byPath("omitted").defaultValue.isEmpty,
        settings.valueType == IntoSchema[Settings].graph,
        byPath("recursive").valueType == IntoSchema[Node].graph
      )
    },
    test("reads current host captures and retains optional positions") {
      var currentSettings = supplied.settings
      val loader          = new ConfigFieldLoader {
        def loadLocal[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): A = {
          val value: TypedSchemaValue = path match {
            case List("settings")  => IntoSchema[Settings].toTyped(currentSettings)
            case List("slots")     => IntoSchema[(Option[Int], Option[Int], String)].toTyped(supplied.slots)
            case List("cleared")   => IntoSchema[Option[String]].toTyped(None)
            case List("omitted")   => IntoSchema[Option[String]].toTyped(None)
            case List("recursive") => IntoSchema[Node].toTyped(supplied.recursive)
            case List("enabled")   => IntoSchema[Boolean].toTyped(false)
            case other             => throw new IllegalArgumentException(other.mkString("."))
          }
          from.fromValue(value.value).fold(throw _, identity)
        }
        def loadSecret[A](path: List[String])(implicit into: IntoSchema[A], from: FromSchema[A]): Secret[A] =
          throw new AssertionError("Static data must not read secrets")
        def loadLocalWire[A](path: List[String], graph: WitSchemaGraph, codec: ConcreteCodec[A]): A =
          throw new AssertionError("Reflective builder uses the model loader")
        def loadSecretWire[A](
          path: List[String],
          graph: WitSchemaGraph,
          handleGraph: WitSchemaGraph,
          codec: ConcreteCodec[A]
        ): Secret[A] =
          throw new AssertionError("Static data must not read secrets")
      }
      val config             = Config(() => ConfigBuilder[Values].build(Nil, loader))
      val settings: Settings = config.settings
      val enabled: Boolean   = config.enabled
      val original           = config.value
      type Alias = Values
      val aliased: Config[Alias] = config
      val aliasedSettings: Settings = aliased.settings
      currentSettings = Settings("updated", UInt(17L))
      assertTrue(
        settings == supplied.settings,
        aliasedSettings == supplied.settings,
        !enabled,
        config.slots == supplied.slots,
        original == supplied,
        config.settings == currentSettings,
        config.value == supplied.copy(settings = currentSettings)
      )
    },
    test("rejects wrong-typed defaults and defaults on managed bindings") {
      val wrong   = scala.util.Try(ConfigSchema[Bad].describe(Nil))
      val managed = scala.util.Try(ConfigSchema[Managed].describe(Nil))
      assertTrue(wrong.isFailure, managed.isFailure)
    },
    test("validates native numeric defaults inside containers") {
      val integer  = scala.util.Try(ConfigSchema[BadInteger].describe(Nil))
      val decimals = scala.util.Try(ConfigSchema[BadDecimals].describe(Nil))
      val valid    = ConfigSchema[NativeNumbers].describe(Nil)
      assertTrue(
        integer.failed.get.getMessage.contains("Invalid config default"),
        decimals.failed.get.getMessage.contains("Invalid config default"),
        valid.map(_.defaultValue) == List(
          Some(SchemaValue.StringValue("123456789012345678901234567890")),
          Some(SchemaValue.ListValue(List(SchemaValue.StringValue("1.25"), SchemaValue.StringValue("-0.5"))))
        )
      )
    },
    test("projects native config names and recursive values without changing ordinary schemas") {
      val declarations  = ConfigSchema[Native].describe(Nil)
      val value         = NativeNode("limits.support_default", Some(NativeNode("tail", None)))
      val overrideValue = ConfigOverride(List("tree"), value)
      val json          = golem.reflection.SchemaRef(overrideValue.value.graph).unpackJson(overrideValue.value.value)
      assertTrue(
        declarations.map(_.path) == List(List("maxChars"), List("tree")),
        ConfigOverride(List("max_chars"), 0).path == List("maxChars"),
        json == Right(
          Json.parseUnsafe(
            "{\"displayName\":\"limits.support_default\",\"next\":{\"displayName\":\"tail\",\"next\":null}}"
          )
        ),
        FromSchema[NativeNode].fromValue(overrideValue.value.value) == Right(value),
        golem.reflection.SchemaRef(IntoSchema[NativeNode].graph).unpackJson(overrideValue.value.value) ==
          Right(
            Json.parseUnsafe(
              "{\"display_name\":\"limits.support_default\",\"next\":{\"display_name\":\"tail\",\"next\":null}}"
            )
          ),
        scala.util.Try(ConfigSchema[Collision].describe(Nil)).isFailure
      )
    },
    test("creation overrides retain optional tuple positions and recursive record values") {
      val slots     = ConfigOverride(List("slots"), supplied.slots)
      val recursive = ConfigOverride(List("recursive"), supplied.recursive)
      assertTrue(
        slots.value.value == SchemaValue.TupleValue(
          List(
            SchemaValue.OptionValue(Some(SchemaValue.S32Value(-4))),
            SchemaValue.OptionValue(None),
            SchemaValue.StringValue("")
          )
        ),
        FromSchema[Node].fromValue(recursive.value.value) == Right(supplied.recursive)
      )
    },
    test("a failed capture read never falls back to a code default") {
      val config = Config[Values](() => throw new IllegalStateException("missing host capture"))
      assertTrue(scala.util.Try(config.enabled).failed.get.getMessage == "missing host capture")
    }
  )
}
