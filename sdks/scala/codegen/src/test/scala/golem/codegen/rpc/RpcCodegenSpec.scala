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

package golem.codegen.rpc

import golem.codegen.discovery.SourceDiscovery
import golem.codegen.ir.AgentSurfaceIR._

class RpcCodegenSpec extends munit.FunSuite {

  private def agent(
    traitFqn: String,
    packageName: String,
    simpleName: String,
    typeName: String = "",
    params: List[ParamSurface] = Nil,
    description: Option[String] = None,
    mode: String = "durable",
    snapshotting: String = "disabled",
    methods: List[MethodSurface] = Nil,
    configFields: List[ConfigFieldSurface] = Nil
  ): AgentSurface =
    AgentSurface(
      traitFqn = traitFqn,
      packageName = packageName,
      simpleName = simpleName,
      typeName = if (typeName.isEmpty) simpleName else typeName,
      constructor = ConstructorSurface(params),
      metadata = AgentMetadataSurface(description, mode, snapshotting),
      methods = methods,
      configFields = configFields
    )

  test("generates client object with package and AgentCompanionBase") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.MyAgent", "example", "MyAgent")),
      existingObjects = Seq.empty
    )

    assertEquals(result.files.size, 1)
    assertEquals(result.warnings.size, 0)

    val content = result.files.head.content
    assert(content.contains("package example"))
    assert(content.contains("object MyAgentClient"))
    assert(content.contains("_root_.golem.AgentCompanionBase[MyAgent]"))
    assert(content.contains("val typeName"))
    assert(content.contains("lazy val agentType"))
  }

  test("skips generation when client object already exists") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.MyAgent", "example", "MyAgent")),
      existingObjects = Seq(
        SourceDiscovery.ExistingObject("MyAgentClient.scala", "example", "MyAgentClient")
      )
    )

    assertEquals(result.files.size, 0)
    assertEquals(result.warnings.size, 1)
    assert(result.warnings.head.message.contains("already exists"))
  }

  test("does not skip when only handwritten trait companion exists") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.MyAgent", "example", "MyAgent")),
      existingObjects = Seq(
        SourceDiscovery.ExistingObject("MyAgent.scala", "example", "MyAgent")
      )
    )

    assertEquals(result.files.size, 1)
    assert(result.files.head.content.contains("object MyAgentClient"))
  }

  test("generates multiple companions") {
    val result = RpcCodegen.generate(
      agents = List(
        agent("a.AgentA", "a", "AgentA", methods = List(MethodSurface("foo", Nil, "Future[Unit]", Nil))),
        agent(
          "b.AgentB",
          "b",
          "AgentB",
          params = List(ParamSurface("name", "String")),
          methods = List(MethodSurface("bar", Nil, "Future[Unit]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    assertEquals(result.files.size, 2)
    assertEquals(result.warnings.size, 0)
  }

  test("file path uses package dirs with Client suffix") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.integrationtests.Counter", "example.integrationtests", "Counter")),
      existingObjects = Seq.empty
    )

    assertEquals(result.files.head.relativePath, "example/integrationtests/CounterClient.scala")
  }

  test("Constructor and bindRemote are private") {
    val result = RpcCodegen.generate(
      agents = List(agent("demo.Test", "demo", "Test", methods = List(MethodSurface("foo", Nil, "Future[Unit]", Nil)))),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("private type Id"), s"Id type should be private:\n$content")
    assert(content.contains("private def bindRemote("), s"bindRemote should be private:\n$content")
  }

  test("no public connect/connectRemote methods") {
    val result = RpcCodegen.generate(
      agents = List(agent("demo.Test", "demo", "Test", methods = List(MethodSurface("foo", Nil, "Future[Unit]", Nil)))),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(!content.contains("def connect("), s"should not have public connect:\n$content")
    assert(!content.contains("def connectPhantom("), s"should not have public connectPhantom:\n$content")
    assert(!content.contains("def connectRemote("), s"should not have connectRemote:\n$content")
    assert(!content.contains("def connectRemotePhantom("), s"should not have connectRemotePhantom:\n$content")
  }

  // ── Remote trait generation tests ─────────────────────────────────────────

  test("generates XRemote trait with per-method classes") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface(
              "process",
              List(ParamSurface("id", "Int"), ParamSurface("msg", "String")),
              "Future[String]",
              List(false, false)
            ),
            MethodSurface("ping", Nil, "Future[Unit]", Nil)
          )
        )
      ),
      existingObjects = Seq.empty
    )

    assertEquals(result.files.size, 1)
    val content = result.files.head.content

    assert(content.contains("trait MyAgentRemote"), s"missing MyAgentRemote in:\n$content")
    assert(content.contains("val process: ProcessRemoteMethod"), s"missing process val in:\n$content")
    assert(content.contains("val ping: PingRemoteMethod"), s"missing ping val in:\n$content")
    assert(content.contains("final class ProcessRemoteMethod"), s"missing ProcessRemoteMethod in:\n$content")
    assert(content.contains("final class PingRemoteMethod"), s"missing PingRemoteMethod in:\n$content")
    assert(
      content.contains("def apply(id: Int, msg: String): _root_.scala.concurrent.Future[String]"),
      s"missing apply in:\n$content"
    )
    assert(
      content.contains("def trigger(id: Int, msg: String): _root_.scala.concurrent.Future[_root_.scala.Unit]"),
      s"missing trigger in:\n$content"
    )
    assert(
      content.contains("def scheduleAt(id: Int, msg: String, when: _root_.golem.Datetime)"),
      s"missing scheduleAt in:\n$content"
    )
  }

  test("no-param method generates apply/trigger/scheduleAt with no args") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(MethodSurface("ping", Nil, "Future[Unit]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def apply(): _root_.scala.concurrent.Future[Unit]"), s"missing apply() in:\n$content")
    assert(
      content.contains("def trigger(): _root_.scala.concurrent.Future[_root_.scala.Unit]"),
      s"missing trigger() in:\n$content"
    )
    assert(content.contains("def scheduleAt(when: _root_.golem.Datetime)"), s"missing scheduleAt(when) in:\n$content")
  }

  test("single-param method packs arg directly") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(MethodSurface("get", List(ParamSurface("key", "String")), "Future[String]", List(false)))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(
      content.contains("AbstractRemoteMethod[MyAgent, String, String]"),
      s"missing correct method type in:\n$content"
    )
    assert(
      content.contains("def apply(key: String): _root_.scala.concurrent.Future[String]"),
      s"missing apply in:\n$content"
    )
  }

  test("multi-param method packs args as Vector") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface(
              "put",
              List(ParamSurface("key", "String"), ParamSurface("value", "Int")),
              "Future[Unit]",
              List(false, false)
            )
          )
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("Vector[_root_.scala.Any]"), s"missing Vector packing in:\n$content")
    assert(content.contains("_root_.scala.Vector[_root_.scala.Any](key, value)"), s"missing packed args in:\n$content")
  }

  test("principal params are filtered out from remote methods") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface(
              "process",
              List(ParamSurface("caller", "Principal"), ParamSurface("data", "String")),
              "Future[String]",
              List(true, false)
            )
          )
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def apply(data: String)"), s"missing filtered apply in:\n$content")
    assert(!content.contains("caller: Principal"), s"principal param should be filtered out:\n$content")
    assert(content.contains("AbstractRemoteMethod[MyAgent, String, String]"), s"wrong method type in:\n$content")
  }

  test("all-principal method generates no-arg apply") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface("whoami", List(ParamSurface("caller", "Principal")), "Future[String]", List(true))
          )
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(
      content.contains("def apply(): _root_.scala.concurrent.Future[String]"),
      s"missing no-arg apply in:\n$content"
    )
    assert(content.contains("_root_.scala.Unit"), s"missing Unit input type in:\n$content")
  }

  test("unwrapFutureType handles various Future forms") {
    assertEquals(RpcCodegen.unwrapFutureType("Future[String]"), "String")
    assertEquals(RpcCodegen.unwrapFutureType("scala.concurrent.Future[Int]"), "Int")
    assertEquals(RpcCodegen.unwrapFutureType("_root_.scala.concurrent.Future[Unit]"), "Unit")
    assertEquals(RpcCodegen.unwrapFutureType("String"), "String")
    assertEquals(RpcCodegen.unwrapFutureType("Unit"), "Unit")
    assertEquals(RpcCodegen.unwrapFutureType("Option[String]"), "Option[String]")
    assertEquals(RpcCodegen.unwrapFutureType("Future[(Int, String)]"), "(Int, String)")
  }

  test("generated method classes extend AbstractRemoteMethod and delegate via helpers") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface(
              "process",
              List(ParamSurface("id", "Int"), ParamSurface("msg", "String")),
              "Future[String]",
              List(false, false)
            )
          )
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(
      content.contains("extends _root_.golem.runtime.rpc.AbstractRemoteMethod["),
      s"missing AbstractRemoteMethod extends in:\n$content"
    )
    assert(content.contains("awaitWith("), s"missing awaitWith delegation in:\n$content")
    assert(content.contains("triggerWith("), s"missing triggerWith delegation in:\n$content")
    assert(content.contains("scheduleWith("), s"missing scheduleWith delegation in:\n$content")
    assert(content.contains("scheduleCancelableWith("), s"missing scheduleCancelableWith delegation in:\n$content")
    assert(!content.contains("resolved.await("), s"should not directly call resolved.await:\n$content")
    assert(!content.contains("resolved.trigger("), s"should not directly call resolved.trigger:\n$content")
    assert(!content.contains("resolved.schedule("), s"should not directly call resolved.schedule:\n$content")
  }

  test("no remote trait generated when methods list is empty") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.MyAgent", "example", "MyAgent")),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(!content.contains("Remote"), s"should not contain Remote when no methods:\n$content")
    assert(!content.contains("bindRemote"), s"should not contain bindRemote when no methods:\n$content")
  }

  test("method with mismatched principalParams length is skipped") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface("good", List(ParamSurface("x", "Int")), "Future[Int]", List(false)),
            MethodSurface("bad", List(ParamSurface("x", "Int")), "Future[Int]", List(false, false))
          )
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("GoodRemoteMethod"), s"missing good method:\n$content")
    assert(!content.contains("BadRemoteMethod"), s"bad method with mismatched params should be skipped:\n$content")
  }

  test("method lookup uses method name string") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(MethodSurface("process", List(ParamSurface("x", "Int")), "Future[Int]", List(false)))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(
      content.contains("""(resolved, "process")"""),
      s"missing method name in AbstractRemoteMethod constructor in:\n$content"
    )
  }

  // ── Mode-aware constructor tests ──────────────────────────────────────────

  test("durable agent without config generates get and phantom but no WithConfig") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          mode = "durable",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def get("), s"missing get in:\n$content")
    assert(content.contains("MyAgentRemote"), s"get should return MyAgentRemote:\n$content")
    assert(!content.contains("def getWithConfig("), s"should not have getWithConfig without config fields:\n$content")
    assert(content.contains("def getPhantom("), s"durable agent should also have getPhantom:\n$content")
    assert(content.contains("def newPhantom("), s"durable agent should also have newPhantom:\n$content")
    assert(
      !content.contains("def getPhantomWithConfig("),
      s"should not have getPhantomWithConfig without config fields:\n$content"
    )
    assert(
      !content.contains("def newPhantomWithConfig("),
      s"should not have newPhantomWithConfig without config fields:\n$content"
    )
  }

  test("durable agent with config fields generates typed WithConfig constructors") {
    val cfgFields = List(
      ConfigFieldSurface(List("appName"), "String"),
      ConfigFieldSurface(List("db", "host"), "String"),
      ConfigFieldSurface(List("db", "port"), "Int")
    )
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.CfgAgent",
          "example",
          "CfgAgent",
          mode = "durable",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil)),
          configFields = cfgFields
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def getWithConfig("), s"missing getWithConfig in:\n$content")
    assert(
      content.contains("appName: _root_.scala.Option[String] = _root_.scala.None"),
      s"missing appName param in:\n$content"
    )
    assert(
      content.contains("dbHost: _root_.scala.Option[String] = _root_.scala.None"),
      s"missing dbHost param in:\n$content"
    )
    assert(
      content.contains("dbPort: _root_.scala.Option[Int] = _root_.scala.None"),
      s"missing dbPort param in:\n$content"
    )
    assert(content.contains("def getPhantomWithConfig("), s"missing getPhantomWithConfig in:\n$content")
    assert(content.contains("def newPhantomWithConfig("), s"missing newPhantomWithConfig in:\n$content")
    assert(
      !content.contains("configOverrides: _root_.scala.List"),
      s"should not use raw ConfigOverride list:\n$content"
    )
  }

  test("WithConfig constructors build ConfigOverride list internally") {
    val cfgFields = List(
      ConfigFieldSurface(List("host"), "String")
    )
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.CfgAgent",
          "example",
          "CfgAgent",
          mode = "durable",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil)),
          configFields = cfgFields
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("ConfigOverride[String]"), s"should build ConfigOverride internally:\n$content")
    assert(content.contains(""""host""""), s"should use path literal:\n$content")
  }

  test("ephemeral agent without config generates getPhantom and newPhantom only") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.EphAgent",
          "example",
          "EphAgent",
          mode = "ephemeral",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def getPhantom("), s"missing getPhantom in:\n$content")
    assert(content.contains("def newPhantom("), s"missing newPhantom in:\n$content")
    assert(content.contains("generateIdempotencyKey"), s"newPhantom should generate UUID:\n$content")
    assert(!content.contains("def get("), s"ephemeral agent should not have get:\n$content")
    assert(!content.contains("def getWithConfig("), s"ephemeral agent should not have getWithConfig:\n$content")
    assert(
      !content.contains("def getPhantomWithConfig("),
      s"should not have getPhantomWithConfig without config fields:\n$content"
    )
    assert(
      !content.contains("def newPhantomWithConfig("),
      s"should not have newPhantomWithConfig without config fields:\n$content"
    )
  }

  test("ephemeral agent with config fields generates phantom WithConfig variants") {
    val cfgFields = List(
      ConfigFieldSurface(List("key"), "String")
    )
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.EphAgent",
          "example",
          "EphAgent",
          mode = "ephemeral",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil)),
          configFields = cfgFields
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def getPhantomWithConfig("), s"missing getPhantomWithConfig in:\n$content")
    assert(content.contains("def newPhantomWithConfig("), s"missing newPhantomWithConfig in:\n$content")
    assert(content.contains("key: _root_.scala.Option[String] = _root_.scala.None"), s"missing key param in:\n$content")
    assert(!content.contains("def get("), s"ephemeral agent should not have get:\n$content")
    assert(!content.contains("def getWithConfig("), s"ephemeral agent should not have getWithConfig:\n$content")
  }

  test("durable agent with constructor params generates get and phantom with unpacked params") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.Counter",
          "example",
          "Counter",
          params = List(ParamSurface("name", "String")),
          mode = "durable",
          methods = List(MethodSurface("inc", Nil, "Future[Int]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def get(name: String): CounterRemote"), s"missing get with params in:\n$content")
    assert(
      content.contains("def getPhantom(name: String, phantom: _root_.golem.Uuid): CounterRemote"),
      s"missing getPhantom with params in:\n$content"
    )
    assert(
      content.contains("def newPhantom(name: String): CounterRemote"),
      s"missing newPhantom with params in:\n$content"
    )
  }

  test("durable agent with constructor params and config fields generates WithConfig with both") {
    val cfgFields = List(
      ConfigFieldSurface(List("host"), "String")
    )
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.Counter",
          "example",
          "Counter",
          params = List(ParamSurface("name", "String")),
          mode = "durable",
          methods = List(MethodSurface("inc", Nil, "Future[Int]", Nil)),
          configFields = cfgFields
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(
      content.contains("def getWithConfig(name: String, host:"),
      s"missing getWithConfig with params in:\n$content"
    )
    assert(
      content.contains("def getPhantomWithConfig(name: String, phantom: _root_.golem.Uuid, host:"),
      s"missing getPhantomWithConfig with params in:\n$content"
    )
    assert(
      content.contains("def newPhantomWithConfig(name: String, host:"),
      s"missing newPhantomWithConfig with params in:\n$content"
    )
  }

  test("ephemeral agent with constructor params generates getPhantom and newPhantom with unpacked params") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.EphAgent",
          "example",
          "EphAgent",
          params = List(ParamSurface("name", "String"), ParamSurface("id", "Int")),
          mode = "ephemeral",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(
      content.contains("def getPhantom(name: String, id: Int, phantom: _root_.golem.Uuid): EphAgentRemote"),
      s"missing getPhantom with params in:\n$content"
    )
    assert(
      content.contains("def newPhantom(name: String, id: Int): EphAgentRemote"),
      s"missing newPhantom with params in:\n$content"
    )
  }

  test("durable agent with unit constructor generates no-param get and phantom constructors") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.Simple",
          "example",
          "Simple",
          mode = "durable",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("def get(): SimpleRemote"), s"missing no-param get in:\n$content")
    assert(
      content.contains("def getPhantom(phantom: _root_.golem.Uuid): SimpleRemote"),
      s"missing no-param getPhantom in:\n$content"
    )
    assert(content.contains("def newPhantom(): SimpleRemote"), s"missing no-param newPhantom in:\n$content")
  }

  test("mode-aware constructors use correct resolve methods") {
    val cfgFields = List(
      ConfigFieldSurface(List("key"), "String")
    )
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          mode = "durable",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil)),
          configFields = cfgFields
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("AgentClientRuntime.resolve["), s"get should use resolve:\n$content")
    assert(
      content.contains("AgentClientRuntime.resolveWithConfig["),
      s"getWithConfig should use resolveWithConfig:\n$content"
    )
    assert(content.contains("resolveWithPhantom["), s"getPhantom should use resolveWithPhantom:\n$content")
    assert(
      content.contains("resolveWithPhantomAndConfig["),
      s"getPhantomWithConfig should use resolveWithPhantomAndConfig:\n$content"
    )
  }

  test("ephemeral mode-aware constructors use correct resolve methods") {
    val cfgFields = List(
      ConfigFieldSurface(List("key"), "String")
    )
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.EphAgent",
          "example",
          "EphAgent",
          mode = "ephemeral",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil)),
          configFields = cfgFields
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(content.contains("resolveWithPhantom["), s"getPhantom should use resolveWithPhantom:\n$content")
    assert(
      content.contains("resolveWithPhantomAndConfig["),
      s"getPhantomWithConfig should use resolveWithPhantomAndConfig:\n$content"
    )
  }

  test("no mode-aware constructors when no methods") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.MyAgent", "example", "MyAgent", mode = "durable")),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    assert(!content.contains("def get("), s"should not have mode-aware constructors without methods:\n$content")
    assert(
      !content.contains("def getWithConfig("),
      s"should not have mode-aware constructors without methods:\n$content"
    )
    assert(!content.contains("def getPhantom("), s"should not have phantom constructors without methods:\n$content")
    assert(!content.contains("def newPhantom("), s"should not have phantom constructors without methods:\n$content")
  }
}
