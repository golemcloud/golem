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

package golem.codegen.discovery

class SourceDiscoverySpec extends munit.FunSuite {

  private def src(path: String, content: String): SourceDiscovery.SourceInput =
    SourceDiscovery.SourceInput(path, content)

  test("discover @agentDefinition trait with no arguments") {
    val code =
      """|package example
         |
         |import golem.runtime.annotations.agentDefinition
         |
         |@agentDefinition()
         |trait MyAgent
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("MyAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val t = result.traits.head
    assertEquals(t.pkg, "example")
    assertEquals(t.name, "MyAgent")
    assertEquals(t.typeName, None)
    assertEquals(t.constructorParams, Nil)
    assertEquals(t.hasDescription, false)
    assertEquals(t.descriptionValue, None)
    assertEquals(t.mode, None)
    assertEquals(t.methods, Nil)
  }

  test("discover @agentDefinition trait with typeName") {
    val code =
      """|package example.templates
         |
         |import golem.runtime.annotations.{agentDefinition, description}
         |
         |@agentDefinition(typeName = "Human")
         |@description("A human agent.")
         |trait HumanAgent {
         |  class Id(val value: String)
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("HumanAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val t = result.traits.head
    assertEquals(t.pkg, "example.templates")
    assertEquals(t.name, "HumanAgent")
    assertEquals(t.typeName, Some("Human"))
    assertEquals(t.constructorParams, List(SourceDiscovery.ConstructorParam("value", "String")))
    assertEquals(t.hasDescription, true)
    assertEquals(t.descriptionValue, Some("A human agent."))
  }

  test("discover @agentDefinition trait with multi-param constructor") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait Shard {
         |  class Id(val tableName: String, val shardId: Int)
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Shard.scala", code)))

    assertEquals(result.traits.size, 1)
    val t = result.traits.head
    assertEquals(
      t.constructorParams,
      List(
        SourceDiscovery.ConstructorParam("tableName", "String"),
        SourceDiscovery.ConstructorParam("shardId", "Int")
      )
    )
  }

  test("discover @agentImplementation class") {
    val code =
      """|package example
         |
         |@agentImplementation()
         |final class ShardImpl(private val tableName: String, private val shardId: Int) extends Shard {
         |  def get(key: String): String = key
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("ShardImpl.scala", code)))

    assertEquals(result.implementations.size, 1)
    val impl = result.implementations.head
    assertEquals(impl.pkg, "example")
    assertEquals(impl.implClass, "ShardImpl")
    assertEquals(impl.traitType, "Shard")
    assertEquals(impl.ctorTypes, List("String", "Int"))
  }

  test("discover top-level objects") {
    val code =
      """|package example
         |
         |object MyAgent
         |
         |object OtherThing {
         |  val x = 1
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("objects.scala", code)))

    assertEquals(result.objects.size, 2)
    assertEquals(result.objects.map(_.name).toSet, Set("MyAgent", "OtherThing"))
    assert(result.objects.forall(_.pkg == "example"))
  }

  test("discover multiple items from single file") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |@description("A counter agent.")
         |trait Counter {
         |  class Id(val value: String)
         |}
         |
         |object Counter
         |
         |@agentImplementation()
         |final class CounterImpl(private val value: String) extends Counter
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Counter.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.implementations.size, 1)
    assertEquals(result.objects.size, 1)
    assertEquals(result.traits.head.name, "Counter")
    assertEquals(result.implementations.head.implClass, "CounterImpl")
    assertEquals(result.objects.head.name, "Counter")
  }

  test("discover from multiple source files") {
    val code1 =
      """|package example.a
         |
         |@agentDefinition()
         |trait AgentA {
         |  class Id(val value: String)
         |}
         |""".stripMargin

    val code2 =
      """|package example.b
         |
         |@agentDefinition(typeName = "BeeAgent")
         |trait AgentB {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(
      Seq(
        src("AgentA.scala", code1),
        src("AgentB.scala", code2)
      )
    )

    assertEquals(result.traits.size, 2)
    assertEquals(result.traits.map(_.name), Seq("AgentA", "AgentB"))
  }

  test("results are sorted by package and name") {
    val code =
      """|package z.pkg
         |
         |@agentDefinition()
         |trait ZAgent {
         |  class Id()
         |}
         |""".stripMargin

    val code2 =
      """|package a.pkg
         |
         |@agentDefinition()
         |trait AAgent {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(
      Seq(
        src("Z.scala", code),
        src("A.scala", code2)
      )
    )

    assertEquals(result.traits.map(_.pkg), Seq("a.pkg", "z.pkg"))
  }

  test("unparseable source produces a warning") {
    val code = "this is not valid scala at all {{{"

    val result = SourceDiscovery.discover(Seq(src("bad.scala", code)))

    assertEquals(result.traits.size, 0)
    assertEquals(result.warnings.size, 1)
    assert(result.warnings.head.message.contains("Failed to parse"))
  }

  test("@agentImplementation class without a package produces no impl") {
    val code =
      """|@agentImplementation()
         |final class BadImpl(private val value: String) extends SomeTrait
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("BadImpl.scala", code)))

    assertEquals(result.implementations.size, 0)
  }

  test("trait without @agentDefinition is not discovered") {
    val code =
      """|package example
         |
         |trait PlainTrait {
         |  def foo(): String
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Plain.scala", code)))

    assertEquals(result.traits.size, 0)
  }

  test("class without @agentImplementation is not discovered") {
    val code =
      """|package example
         |
         |final class PlainClass(val x: Int) extends SomeTrait
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Plain.scala", code)))

    assertEquals(result.implementations.size, 0)
  }

  test("description without value") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |@description()
         |trait NoDescValue {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("NoDesc.scala", code)))

    assertEquals(result.traits.size, 1)
    val t = result.traits.head
    assertEquals(t.hasDescription, true)
    assertEquals(t.descriptionValue, None)
  }

  test("discover trait with fully qualified annotation") {
    val code =
      """|package example
         |
         |@golem.runtime.annotations.agentDefinition()
         |trait FqnAgent
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Fqn.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.traits.head.name, "FqnAgent")
  }

  test("extract mode from @agentDefinition named argument") {
    val code =
      """|package example
         |
         |@agentDefinition(mode = DurabilityMode.Ephemeral)
         |trait EphemeralAgent {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Ephemeral.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.traits.head.mode, Some("ephemeral"))
  }

  test("extract mode durable from @agentDefinition named argument") {
    val code =
      """|package example
         |
         |@agentDefinition(mode = DurabilityMode.Durable)
         |trait DurableAgent {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Durable.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.traits.head.mode, Some("durable"))
  }

  test("mode defaults to None when not specified") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait DefaultAgent {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Default.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.traits.head.mode, None)
  }

  test("extract mode from imported enum value (Ident)") {
    val code =
      """|package example
         |
         |import DurabilityMode.Ephemeral
         |
         |@agentDefinition(mode = Ephemeral)
         |trait ImportedModeAgent {
         |  class Id()
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("Imported.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.traits.head.mode, Some("ephemeral"))
  }

  test("extract methods from trait") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait MethodAgent {
         |  class Id()
         |  def get(key: String): String
         |  def put(key: String, value: Int): Unit
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("MethodAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val methods = result.traits.head.methods
    assertEquals(methods.size, 2)

    assertEquals(methods(0).name, "get")
    assertEquals(methods(0).params, List(SourceDiscovery.ConstructorParam("key", "String")))
    assertEquals(methods(0).returnTypeExpr, "String")

    assertEquals(methods(1).name, "put")
    assertEquals(
      methods(1).params,
      List(
        SourceDiscovery.ConstructorParam("key", "String"),
        SourceDiscovery.ConstructorParam("value", "Int")
      )
    )
    assertEquals(methods(1).returnTypeExpr, "Unit")
  }

  test("Constructor class members are excluded from methods list") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait MixedAgent {
         |  class Id(val value: String)
         |  def get(key: String): String
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("MixedAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val methods = result.traits.head.methods
    assertEquals(methods.size, 1)
    assertEquals(methods.head.name, "get")
  }

  test("detect Principal param type") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait PrincipalAgent {
         |  class Id()
         |  def foo(principal: Principal): String
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("PrincipalAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val methods = result.traits.head.methods
    assertEquals(methods.size, 1)
    assertEquals(methods.head.principalParams, List(true))
  }

  test("detect golem.Principal param type") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait QualifiedPrincipalAgent {
         |  class Id()
         |  def bar(p: golem.Principal, key: String): String
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("QualifiedPrincipal.scala", code)))

    assertEquals(result.traits.size, 1)
    val methods = result.traits.head.methods
    assertEquals(methods.size, 1)
    assertEquals(methods.head.principalParams, List(true, false))
  }

  test("extract config fields from AgentConfig trait parent") {
    val code =
      """|package example
         |
         |import golem.config.{AgentConfig, Secret}
         |
         |final case class DbConfig(
         |  host: String,
         |  port: Int,
         |  password: Secret[String]
         |)
         |
         |final case class MyAppConfig(
         |  appName: String,
         |  apiKey: Secret[String],
         |  db: DbConfig
         |)
         |
         |@agentDefinition()
         |trait ConfigAgent extends BaseAgent with AgentConfig[MyAppConfig] {
         |  class Id(val value: String)
         |  def greet(): Future[String]
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("ConfigAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val t = result.traits.head
    assertEquals(t.configFields.size, 3)
    assertEquals(t.configFields(0), SourceDiscovery.ConfigField(List("appName"), "String"))
    assertEquals(t.configFields(1), SourceDiscovery.ConfigField(List("db", "host"), "String"))
    assertEquals(t.configFields(2), SourceDiscovery.ConfigField(List("db", "port"), "Int"))
  }

  test("agent without AgentConfig has no config fields") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait SimpleAgent {
         |  class Id()
         |  def hello(): String
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("SimpleAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    assertEquals(result.traits.head.configFields, Nil)
  }

  test("config fields from multiple source files") {
    val configCode =
      """|package example
         |
         |final case class AppConfig(
         |  host: String,
         |  port: Int
         |)
         |""".stripMargin

    val agentCode =
      """|package example
         |
         |@agentDefinition()
         |trait MyAgent extends BaseAgent with AgentConfig[AppConfig] {
         |  class Id(val name: String)
         |  def hello(): String
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(
      Seq(
        src("AppConfig.scala", configCode),
        src("MyAgent.scala", agentCode)
      )
    )

    assertEquals(result.traits.size, 1)
    val t = result.traits.head
    assertEquals(t.configFields.size, 2)
    assertEquals(t.configFields(0), SourceDiscovery.ConfigField(List("host"), "String"))
    assertEquals(t.configFields(1), SourceDiscovery.ConfigField(List("port"), "Int"))
  }

  test("concrete methods with explicit return type are discovered") {
    val code =
      """|package example
         |
         |@agentDefinition()
         |trait ConcreteAgent {
         |  class Id()
         |  def hello(name: String): String = s"Hello, $name"
         |}
         |""".stripMargin

    val result = SourceDiscovery.discover(Seq(src("ConcreteAgent.scala", code)))

    assertEquals(result.traits.size, 1)
    val methods = result.traits.head.methods
    assertEquals(methods.size, 1)
    assertEquals(methods.head.name, "hello")
    assertEquals(methods.head.returnTypeExpr, "String")
  }
}
