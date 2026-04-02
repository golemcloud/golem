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

package golem

import golem.data.multimodal.Multimodal
import golem.data.UnstructuredBinaryValue
import golem.data.UnstructuredTextValue
import golem.data.unstructured.{AllowedLanguages, AllowedMimeTypes, BinarySegment, TextSegment}
import golem.runtime.autowire.AgentImplementation
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future

object AgentDefinitionCompileSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // Constructor patterns
  // ---------------------------------------------------------------------------

  @agentDefinition("unit-ctor-agent", mode = DurabilityMode.Durable)
  trait UnitCtorAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  @agentDefinition("string-ctor-agent")
  trait StringCtorAgent extends BaseAgent {
    class Id(val value: String)
    def echo(): Future[String]
  }

  @agentDefinition("case-class-ctor-agent")
  trait CaseClassCtorAgent extends BaseAgent {
    class Id(val host: String, val port: Int)
    def info(): Future[String]
  }

  final case class MyConfig(host: String, port: Int)
  object MyConfig { implicit val schema: Schema[MyConfig] = Schema.derived }

  @agentDefinition("tuple2-ctor-agent")
  trait Tuple2CtorAgent extends BaseAgent {
    class Id(val arg0: String, val arg1: Int)
    def combined(): Future[String]
  }

  @agentDefinition("tuple3-ctor-agent")
  trait Tuple3CtorAgent extends BaseAgent {
    class Id(val arg0: String, val arg1: Int, val arg2: Boolean)
    def all(): Future[String]
  }

  @agentDefinition("tuple4-ctor-agent")
  trait Tuple4CtorAgent extends BaseAgent {
    class Id(val arg0: String, val arg1: Int, val arg2: Boolean, val arg3: Double)
    def data(): Future[String]
  }

  @agentDefinition("tuple5-ctor-agent")
  trait Tuple5CtorAgent extends BaseAgent {
    class Id(val arg0: String, val arg1: Int, val arg2: Boolean, val arg3: Double, val arg4: Long)
    def data(): Future[String]
  }

  // ---------------------------------------------------------------------------
  // Method return type patterns
  // ---------------------------------------------------------------------------

  @agentDefinition("return-types-agent")
  trait ReturnTypesAgent extends BaseAgent {
    class Id()
    def asyncString(): Future[String]
    def asyncInt(): Future[Int]
    def asyncOption(): Future[Option[String]]
    def asyncList(): Future[List[Int]]
    def asyncCaseClass(): Future[MyConfig]
    def syncString(): String
    def syncInt(): Int
    def syncUnit(): Unit
  }

  // ---------------------------------------------------------------------------
  // Method parameter patterns
  // ---------------------------------------------------------------------------

  final case class Nested(inner: String, count: Int)
  object Nested { implicit val schema: Schema[Nested] = Schema.derived }

  @agentDefinition("param-types-agent")
  trait ParamTypesAgent extends BaseAgent {
    class Id()
    def singlePrimitive(s: String): Future[String]
    def multipleParams(a: String, b: Int, c: Boolean): Future[String]
    def caseClassParam(config: MyConfig): Future[String]
    def optionParam(value: Option[String]): Future[String]
    def listParam(values: List[Int]): Future[String]
    def nestedParam(n: Nested): Future[String]
  }

  // ---------------------------------------------------------------------------
  // Kitchen-sink agent: many method signatures (mirrors Rust SDK's Echo agent)
  // ---------------------------------------------------------------------------

  final case class KitchenPayload(tag: String, count: Int)
  object KitchenPayload { implicit val schema: Schema[KitchenPayload] = Schema.derived }

  @agentDefinition("kitchen-sink-agent")
  @description("Agent with many method signature patterns.")
  trait KitchenSinkAgent extends BaseAgent {
    class Id(val value: String)
    def echoString(message: String): Future[String]
    def echoInt(value: Int): Future[Int]
    def echoBoolean(flag: Boolean): Future[Boolean]
    def echoLong(value: Long): Future[Long]
    def echoDouble(value: Double): Future[Double]
    def echoFloat(value: Float): Future[Float]

    def echoOption(opt: Option[String]): Future[Option[String]]
    def echoOptionInt(opt: Option[Int]): Future[Option[Int]]
    def echoList(items: List[String]): Future[List[String]]
    def echoListInt(items: List[Int]): Future[List[Int]]
    def echoCaseClass(payload: KitchenPayload): Future[KitchenPayload]
    def echoNested(n: Nested): Future[Nested]

    def multiParam(a: String, b: Int, c: Boolean): Future[String]
    def multiParamComplex(name: String, config: MyConfig, tags: List[String]): Future[String]

    def syncVoid(): Unit
    def syncReturn(): String
    def asyncVoid(): Future[Unit]

    @description("A described method.")
    @prompt("Use this to echo with metadata.")
    def describedMethod(input: String): Future[String]
  }

  @agentImplementation()
  final class KitchenSinkAgentImpl(private val name: String) extends KitchenSinkAgent {
    override def echoString(message: String): Future[String]                                           = Future.successful(message)
    override def echoInt(value: Int): Future[Int]                                                      = Future.successful(value)
    override def echoBoolean(flag: Boolean): Future[Boolean]                                           = Future.successful(flag)
    override def echoLong(value: Long): Future[Long]                                                   = Future.successful(value)
    override def echoDouble(value: Double): Future[Double]                                             = Future.successful(value)
    override def echoFloat(value: Float): Future[Float]                                                = Future.successful(value)
    override def echoOption(opt: Option[String]): Future[Option[String]]                               = Future.successful(opt)
    override def echoOptionInt(opt: Option[Int]): Future[Option[Int]]                                  = Future.successful(opt)
    override def echoList(items: List[String]): Future[List[String]]                                   = Future.successful(items)
    override def echoListInt(items: List[Int]): Future[List[Int]]                                      = Future.successful(items)
    override def echoCaseClass(payload: KitchenPayload): Future[KitchenPayload]                        = Future.successful(payload)
    override def echoNested(n: Nested): Future[Nested]                                                 = Future.successful(n)
    override def multiParam(a: String, b: Int, c: Boolean): Future[String]                             = Future.successful(s"$a-$b-$c")
    override def multiParamComplex(name: String, config: MyConfig, tags: List[String]): Future[String] =
      Future.successful(s"$name-${config.host}-${tags.mkString(",")}")
    override def syncVoid(): Unit                               = ()
    override def syncReturn(): String                           = name
    override def asyncVoid(): Future[Unit]                      = Future.successful(())
    override def describedMethod(input: String): Future[String] = Future.successful(s"described: $input")
  }

  // ---------------------------------------------------------------------------
  // Annotation patterns
  // ---------------------------------------------------------------------------

  @agentDefinition("explicit-name-agent")
  @description("An agent with explicit type name.")
  trait ExplicitNameAgent extends BaseAgent {
    class Id()
    @description("Says hello.")
    @prompt("Greet the user warmly.")
    def greet(name: String): Future[String]
  }

  @agentDefinition("ephemeral-agent", mode = DurabilityMode.Ephemeral)
  trait EphemeralAgent extends BaseAgent {
    class Id(val value: String)
    def process(): Future[String]
  }

  // ---------------------------------------------------------------------------
  // Implementation patterns
  // ---------------------------------------------------------------------------

  @agentImplementation()
  final class UnitCtorAgentImpl() extends UnitCtorAgent {
    override def ping(): Future[String] = Future.successful("pong")
  }

  @agentImplementation()
  final class StringCtorAgentImpl(private val name: String) extends StringCtorAgent {
    override def echo(): Future[String] = Future.successful(name)
  }

  @agentImplementation()
  final class CaseClassCtorAgentImpl(private val host: String, private val port: Int) extends CaseClassCtorAgent {
    override def info(): Future[String] = Future.successful(s"$host:$port")
  }

  @agentImplementation()
  final class Tuple2CtorAgentImpl(private val name: String, private val id: Int) extends Tuple2CtorAgent {
    override def combined(): Future[String] = Future.successful(s"$name-$id")
  }

  @agentImplementation()
  final class ReturnTypesAgentImpl() extends ReturnTypesAgent {
    override def asyncString(): Future[String]         = Future.successful("hello")
    override def asyncInt(): Future[Int]               = Future.successful(42)
    override def asyncOption(): Future[Option[String]] = Future.successful(Some("x"))
    override def asyncList(): Future[List[Int]]        = Future.successful(List(1, 2, 3))
    override def asyncCaseClass(): Future[MyConfig]    = Future.successful(MyConfig("h", 80))
    override def syncString(): String                  = "sync"
    override def syncInt(): Int                        = 7
    override def syncUnit(): Unit                      = ()
  }

  // ---------------------------------------------------------------------------
  // Factory constructor pattern
  // ---------------------------------------------------------------------------

  @agentDefinition("factory-ctor-agent")
  trait FactoryCtorAgent extends BaseAgent {
    class Id(val host: String, val port: Int)
    def info(): Future[String]
  }

  @agentImplementation()
  final class FactoryCtorAgentImpl(private val host: String, private val port: Int) extends FactoryCtorAgent {
    override def info(): Future[String] = Future.successful(s"$host:$port")
  }

  // ---------------------------------------------------------------------------
  // Agent with zero methods
  // ---------------------------------------------------------------------------

  @agentDefinition("no-methods-agent")
  trait NoMethodsAgent extends BaseAgent {
    class Id(val value: String)
  }

  @agentImplementation()
  final class NoMethodsAgentImpl(private val name: String) extends NoMethodsAgent

  // ---------------------------------------------------------------------------
  // Agent with single method
  // ---------------------------------------------------------------------------

  @agentDefinition("single-method-agent")
  trait SingleMethodAgent extends BaseAgent {
    class Id()
    def only(): Future[String]
  }

  @agentImplementation()
  final class SingleMethodAgentImpl() extends SingleMethodAgent {
    override def only(): Future[String] = Future.successful("only")
  }

  // ---------------------------------------------------------------------------
  // Agent with Multimodal, TextSegment, BinarySegment method parameters
  // ---------------------------------------------------------------------------

  final case class MultimodalPayload(text: String, count: Int)
  object MultimodalPayload { implicit val schema: Schema[MultimodalPayload] = Schema.derived }

  sealed trait SupportedLang
  object SupportedLang {
    implicit val allowed: AllowedLanguages[SupportedLang] = new AllowedLanguages[SupportedLang] {
      override val codes: Option[List[String]] = Some(List("en", "es"))
    }
  }

  sealed trait SupportedMime
  object SupportedMime {
    implicit val allowed: AllowedMimeTypes[SupportedMime] = new AllowedMimeTypes[SupportedMime] {
      override val mimeTypes: Option[List[String]] = Some(List("image/png", "application/json"))
    }
  }

  @agentDefinition("multimodal-agent")
  @description("Agent with multimodal and unstructured type methods.")
  trait MultimodalAgent extends BaseAgent {
    class Id()
    def echoMultimodal(input: Multimodal[MultimodalPayload]): Future[Multimodal[MultimodalPayload]]
    def echoText(input: TextSegment[SupportedLang]): Future[TextSegment[SupportedLang]]
    def echoTextAny(input: TextSegment[AllowedLanguages.Any]): Future[TextSegment[AllowedLanguages.Any]]
    def echoBinary(input: BinarySegment[SupportedMime]): Future[BinarySegment[SupportedMime]]
    def echoBinaryAny(input: BinarySegment[AllowedMimeTypes.Any]): Future[BinarySegment[AllowedMimeTypes.Any]]
  }

  @agentImplementation()
  final class MultimodalAgentImpl() extends MultimodalAgent {
    override def echoMultimodal(input: Multimodal[MultimodalPayload]): Future[Multimodal[MultimodalPayload]] =
      Future.successful(input)
    override def echoText(input: TextSegment[SupportedLang]): Future[TextSegment[SupportedLang]] =
      Future.successful(input)
    override def echoTextAny(input: TextSegment[AllowedLanguages.Any]): Future[TextSegment[AllowedLanguages.Any]] =
      Future.successful(input)
    override def echoBinary(input: BinarySegment[SupportedMime]): Future[BinarySegment[SupportedMime]] =
      Future.successful(input)
    override def echoBinaryAny(
      input: BinarySegment[AllowedMimeTypes.Any]
    ): Future[BinarySegment[AllowedMimeTypes.Any]] =
      Future.successful(input)
  }

  // ---------------------------------------------------------------------------
  // Shared registrations (each agent type can only be registered once)
  // ---------------------------------------------------------------------------

  private lazy val unitCtorDefn      = AgentImplementation.registerClass[UnitCtorAgent, UnitCtorAgentImpl]
  private lazy val stringCtorDefn    = AgentImplementation.registerClass[StringCtorAgent, StringCtorAgentImpl]
  private lazy val caseClassCtorDefn = AgentImplementation.registerClass[CaseClassCtorAgent, CaseClassCtorAgentImpl]
  private lazy val tuple2CtorDefn    = AgentImplementation.registerClass[Tuple2CtorAgent, Tuple2CtorAgentImpl]
  private lazy val returnTypesDefn   = AgentImplementation.registerClass[ReturnTypesAgent, ReturnTypesAgentImpl]
  private lazy val kitchenSinkDefn   = AgentImplementation.registerClass[KitchenSinkAgent, KitchenSinkAgentImpl]
  private lazy val factoryCtorDefn   = AgentImplementation.registerClass[FactoryCtorAgent, FactoryCtorAgentImpl]
  private lazy val noMethodsDefn     = AgentImplementation.registerClass[NoMethodsAgent, NoMethodsAgentImpl]
  private lazy val singleMethodDefn  = AgentImplementation.registerClass[SingleMethodAgent, SingleMethodAgentImpl]
  private lazy val multimodalDefn    = AgentImplementation.registerClass[MultimodalAgent, MultimodalAgentImpl]

  // ---------------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------------

  def spec = suite("AgentDefinitionCompileSpec")(
    test("no-arg constructor compiles") {
      assertTrue(unitCtorDefn.methodMetadata.nonEmpty)
    },
    test("single-param Constructor compiles") {
      assertTrue(stringCtorDefn.methodMetadata.nonEmpty)
    },
    test("multi-param Constructor compiles") {
      assertTrue(caseClassCtorDefn.methodMetadata.nonEmpty)
    },
    test("tuple-style Constructor compiles") {
      assertTrue(tuple2CtorDefn.methodMetadata.nonEmpty)
    },
    test("async and sync return types compile") {
      val methodNames = returnTypesDefn.methodMetadata.map(_.metadata.name).toSet
      assertTrue(
        methodNames.contains("asyncString"),
        methodNames.contains("asyncInt"),
        methodNames.contains("asyncOption"),
        methodNames.contains("asyncList"),
        methodNames.contains("asyncCaseClass"),
        methodNames.contains("syncString"),
        methodNames.contains("syncInt"),
        methodNames.contains("syncUnit")
      )
    },
    test("method count is correct for ReturnTypesAgent") {
      assertTrue(returnTypesDefn.methodMetadata.size == 8)
    },
    test("kitchen-sink agent with 18 methods registers correctly") {
      assertTrue(kitchenSinkDefn.methodMetadata.size == 18)
    },
    test("kitchen-sink agent method names are all present") {
      val methodNames = kitchenSinkDefn.methodMetadata.map(_.metadata.name).toSet
      val expected    = Set(
        "echoString",
        "echoInt",
        "echoBoolean",
        "echoLong",
        "echoDouble",
        "echoFloat",
        "echoOption",
        "echoOptionInt",
        "echoList",
        "echoListInt",
        "echoCaseClass",
        "echoNested",
        "multiParam",
        "multiParamComplex",
        "syncVoid",
        "syncReturn",
        "asyncVoid",
        "describedMethod"
      )
      assertTrue(methodNames == expected)
    },
    test("kitchen-sink agent described method has annotations") {
      val m = kitchenSinkDefn.methodMetadata.find(_.metadata.name == "describedMethod").get
      assertTrue(
        m.metadata.description.contains("A described method."),
        m.metadata.prompt.contains("Use this to echo with metadata.")
      )
    },
    test("@agentDefinition mode defaults and overrides") {
      assertTrue(
        new agentDefinition(mode = DurabilityMode.Durable).mode == DurabilityMode.Durable,
        new agentDefinition(mode = DurabilityMode.Ephemeral).mode == DurabilityMode.Ephemeral
      )
    },
    test("@description and @prompt store their values") {
      assertTrue(
        new description("test description").value == "test description",
        new prompt("test prompt").value == "test prompt"
      )
    },
    test("register with factory constructor compiles") {
      assertTrue(
        factoryCtorDefn.methodMetadata.nonEmpty,
        factoryCtorDefn.typeName == "factory-ctor-agent"
      )
    },
    test("agent with zero methods (only constructor) compiles and registers") {
      assertTrue(
        noMethodsDefn.methodMetadata.isEmpty,
        noMethodsDefn.typeName == "no-methods-agent"
      )
    },
    test("agent with single method compiles and registers") {
      assertTrue(
        singleMethodDefn.methodMetadata.size == 1,
        singleMethodDefn.methodMetadata.head.metadata.name == "only"
      )
    },
    test("multimodal agent compiles and registers with 5 methods") {
      val names = multimodalDefn.methodMetadata.map(_.metadata.name).toSet
      assertTrue(
        multimodalDefn.methodMetadata.size == 5,
        names == Set("echoMultimodal", "echoText", "echoTextAny", "echoBinary", "echoBinaryAny")
      )
    },
    test("Multimodal wraps and unwraps payload") {
      val payload = MultimodalPayload("hello", 1)
      val mm      = Multimodal(payload)
      assertTrue(mm.value == payload)
    },
    test("TextSegment.inline sets data and language code") {
      val seg = TextSegment.inline[SupportedLang]("hello", Some("en"))
      seg.value match {
        case UnstructuredTextValue.Inline(data, lang) =>
          assertTrue(data == "hello", lang.contains("en"))
        case _ => throw new RuntimeException("expected Inline")
      }
    },
    test("TextSegment.url sets URL") {
      val seg = TextSegment.url[SupportedLang]("http://example.com/text.txt")
      seg.value match {
        case UnstructuredTextValue.Url(u) => assertTrue(u == "http://example.com/text.txt")
        case _                            => throw new RuntimeException("expected Url")
      }
    },
    test("BinarySegment.inline sets data and MIME type") {
      val seg = BinarySegment.inline[SupportedMime](Array[Byte](1, 2), "image/png")
      seg.value match {
        case UnstructuredBinaryValue.Inline(data, mime) =>
          assertTrue(data.toList == List[Byte](1, 2), mime == "image/png")
        case _ => throw new RuntimeException("expected Inline")
      }
    },
    test("BinarySegment.url sets URL") {
      val seg = BinarySegment.url[SupportedMime]("http://example.com/data.png")
      seg.value match {
        case UnstructuredBinaryValue.Url(u) => assertTrue(u == "http://example.com/data.png")
        case _                              => throw new RuntimeException("expected Url")
      }
    }
  )
}
