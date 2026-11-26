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
    assert(content.contains("_root_.golem.AgentCompanionBase[_root_.example.MyAgent]"))
    assert(content.contains("val typeName"))
    assert(content.contains("lazy val agentType"))
  }

  test("source package clauses must form the agent package") {
    val result = RpcCodegen.generate(
      agents = List(
        agent("example.MyAgent", "example", "MyAgent")
          .copy(sourceScopes = List(SourceScopeSurface("other", List("import java.util.UUID"))))
      ),
      existingObjects = Seq.empty
    )

    assertEquals(result.files.size, 0)
    assertEquals(result.errors.map(_.message.contains("do not form its package example")), List(true))
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
      content.contains("AbstractWireRemoteMethod[_root_.example.MyAgent, String, String]"),
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

  test("context params are filtered out from remote methods") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface(
              "process",
              List(ParamSurface("caller", "InvocationContext"), ParamSurface("data", "String")),
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
    assert(!content.contains("caller: InvocationContext"), s"context param should be filtered out:\n$content")
    assert(
      content.contains("AbstractWireRemoteMethod[_root_.example.MyAgent, String, String]"),
      s"wrong method type in:\n$content"
    )
  }

  test("all-context method generates no-arg apply") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface("whoami", List(ParamSurface("caller", "InvocationContext")), "Future[String]", List(true))
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
    assertEquals(
      RpcCodegen.unwrapFutureType("""Future[Either[String, Option[UnstructuredBinary["image/png" | "image/jpeg"]]]]"""),
      """Either[String, Option[UnstructuredBinary["image/png" | "image/jpeg"]]]"""
    )
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
      content.contains("extends _root_.golem.runtime.rpc.AbstractWireRemoteMethod["),
      s"missing AbstractWireRemoteMethod extends in:\n$content"
    )
    assert(content.contains("awaitWith("), s"missing awaitWith delegation in:\n$content")
    assert(content.contains("triggerWith("), s"missing triggerWith delegation in:\n$content")
    assert(content.contains("scheduleWith("), s"missing scheduleWith delegation in:\n$content")
    assert(content.contains("scheduleCancelableWith("), s"missing scheduleCancelableWith delegation in:\n$content")
    assert(!content.contains("resolved.await("), s"should not directly call resolved.await:\n$content")
    assert(!content.contains("resolved.trigger("), s"should not directly call resolved.trigger:\n$content")
    assert(!content.contains("resolved.schedule("), s"should not directly call resolved.schedule:\n$content")
  }

  private def classBody(content: String, className: String, indent: String = "  "): String = {
    val start = content.indexOf(s"${indent}final class $className ")
    assert(start >= 0, s"missing class $className in:\n$content")
    content.substring(start, content.indexOf(s"\n$indent}\n", start))
  }

  private def generatedClient(mode: String, methodNames: String*): RpcCodegen.Result =
    RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          mode = mode,
          methods = methodNames.toList.map(name =>
            MethodSurface(
              name,
              List(ParamSurface("id", "Int"), ParamSurface("msg", "String")),
              "Future[String]",
              List(false, false)
            )
          )
        )
      ),
      existingObjects = Seq.empty
    )

  test("scope cards attach only to awaited calls, per call and as a client default") {
    def generated(mode: String): String = generatedClient(mode, "process").files.head.content

    val card    = "_root_.golem.host.PermissionsApi.ScopeCard"
    val durable = generated("durable")
    val method  = classBody(durable, "ProcessRemoteMethod")
    val scoped  = classBody(durable.substring(durable.indexOf(method)), "GolemScopeCardCall", indent = "    ")
    assert(
      method.contains(s"def withScopeCard(card: $card): GolemScopeCardCall =") &&
        method.contains("new GolemScopeCardCall(resolved.withScopeCard(card))"),
      s"missing per-call scope card in:\n$method"
    )
    assert(scoped.contains("def apply(id: Int, msg: String): _root_.scala.concurrent.Future[String] ="), scoped)
    assert(scoped.contains("awaitWith(_root_.scala.Vector[_root_.scala.Any](id, msg))"), scoped)
    assert(scoped.contains("cancelableAwaitWith("), scoped)
    assert(!scoped.contains("trigger"), s"scoped calls must not offer trigger:\n$scoped")
    assert(!scoped.contains("schedule"), s"scoped calls must not offer schedule:\n$scoped")
    assert(!durable.contains("PermissionCard"), s"scope-card APIs must accept only ScopeCard:\n$durable")
    assert(durable.contains(s"def withDefaultScopeCard(card: $card): MyAgentRemote\n"), durable)
    assert(durable.contains("bindRemote(resolved.withScopeCard(card))"), durable)
    assert(durable.contains("bindRemote(resolved.withoutScopeCard)"), durable)

    val ephemeral       = generated("ephemeral")
    val ephemeralMethod = classBody(ephemeral, "ProcessRemoteMethod")
    val ephemeralScoped =
      classBody(ephemeral.substring(ephemeral.indexOf(ephemeralMethod)), "GolemScopeCardCall", indent = "    ")
    assert(ephemeralScoped.contains("awaitWithMetadata("), ephemeralScoped)
    assert(ephemeralScoped.contains("cancelableAwaitWithMetadata("), ephemeralScoped)
    assert(!ephemeralScoped.contains("trigger"), ephemeralScoped)
  }

  test("a method named like another method plus Scoped gets distinct generated classes") {
    val result  = generatedClient("durable", "process", "processScoped")
    val content = result.files.head.content
    assert(result.errors.isEmpty, result.errors.toString)
    assertEquals(content.split("final class ProcessRemoteMethod ", -1).length - 1, 1, content)
    assertEquals(content.split("final class ProcessScopedRemoteMethod ", -1).length - 1, 1, content)
    assertEquals(content.split("    final class GolemScopeCardCall ", -1).length - 1, 2, content)
  }

  private def clientWithSignature(paramType: String, returnType: String): RpcCodegen.Result =
    RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(MethodSurface("run", List(ParamSurface("s", paramType)), returnType, List(false)))
        )
      ),
      existingObjects = Seq.empty
    )

  test("user types named like a short scoped-call class keep their meaning in generated signatures") {
    val result  = clientWithSignature("Scoped", "Future[Scoped]")
    val content = result.files.head.content
    assert(result.errors.isEmpty, result.errors.toString)
    assert(content.contains("def apply(s: Scoped): _root_.scala.concurrent.Future[Scoped] ="), content)
    assert(!content.contains("class Scoped "), content)
  }

  test("a signature that uses the bare scoped-call class name fails generation clearly") {
    List(
      ("GolemScopeCardCall", "Future[Int]"),
      ("Int", "Future[GolemScopeCardCall]"),
      ("Option[GolemScopeCardCall]", "Future[Unit]"),
      ("`GolemScopeCardCall`", "Future[Int]"),
      ("Int", "Future[Either[String, `GolemScopeCardCall`]]")
    ).foreach { case (paramType, returnType) =>
      val result = clientWithSignature(paramType, returnType)
      assert(result.files.isEmpty, s"$paramType => $returnType")
      assertEquals(result.errors.size, 1)
      assert(result.errors.head.message.contains("GolemScopeCardCall"), result.errors.head.message)
    }
    List(
      ("example.GolemScopeCardCall", "Future[Int]"),
      ("GolemScopeCardCallId", "Future[MyGolemScopeCardCall]")
    ).foreach { case (paramType, returnType) =>
      val result = clientWithSignature(paramType, returnType)
      assert(result.errors.isEmpty, s"$paramType => $returnType: ${result.errors}")
      assertEquals(result.files.size, 1)
    }
  }

  test("name checks run only for clients that are generated") {
    val handwritten       = Seq(SourceDiscovery.ExistingObject("MyAgentClient.scala", "example", "MyAgentClient"))
    val handwrittenClient = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(MethodSurface("withDefaultScopeCard", Nil, "Future[GolemScopeCardCall]", Nil))
        )
      ),
      existingObjects = handwritten
    )
    assert(handwrittenClient.errors.isEmpty, handwrittenClient.errors.toString)
    assertEquals(handwrittenClient.warnings.size, 1)
    // A method without valid context flags gets no wrapper, so its names are free.
    val skippedMethod = RpcCodegen.generate(
      agents = List(
        agent(
          "example.MyAgent",
          "example",
          "MyAgent",
          methods = List(
            MethodSurface("process", Nil, "Future[Int]", Nil),
            MethodSurface("withDefaultScopeCard", List(ParamSurface("x", "GolemScopeCardCall")), "Future[Int]", Nil)
          )
        )
      ),
      existingObjects = Seq.empty
    )
    assert(skippedMethod.errors.isEmpty, skippedMethod.errors.toString)
    assertEquals(skippedMethod.files.size, 1)
  }

  test("agent methods cannot use the names of the generated client members") {
    RpcCodegen.ReservedRemoteMemberNames.foreach { reserved =>
      val result = generatedClient("durable", "process", reserved)
      assert(result.files.isEmpty, s"no client may be generated for a reserved method name $reserved")
      assertEquals(result.errors.size, 1)
      assert(result.errors.head.message.contains(reserved), result.errors.head.message)
    }
    assertEquals(
      RpcCodegen.ReservedRemoteMemberNames,
      Set("withDefaultScopeCard", "withoutDefaultScopeCard", "agentId")
    )
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

  test("method with mismatched contextParams length is skipped") {
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

  private def durableClient(
    params: List[ParamSurface] = Nil,
    configFields: List[ConfigFieldSurface] = Nil,
    mode: String = "durable"
  ): String =
    RpcCodegen
      .generate(
        agents = List(
          agent(
            "example.Counter",
            "example",
            "Counter",
            params = params,
            mode = mode,
            methods = List(MethodSurface("inc", Nil, "Future[Int]", Nil)),
            configFields = configFields
          )
        ),
        existingObjects = Seq.empty
      )
      .files
      .head
      .content

  private val removedConstructors =
    List("get", "getWithConfig", "getPhantom", "newPhantom", "getPhantomWithConfig", "newPhantomWithConfig")

  test("durable agent generates upsert, create and getById and no removed constructors") {
    val content = durableClient(params = List(ParamSurface("name", "String")))
    assert(content.contains("def upsert(name: String): CounterRemote"), s"missing upsert in:\n$content")
    assert(
      content.contains(
        "def create(name: String, keyed: _root_.scala.Boolean = false, env: _root_.scala.Predef.Map[_root_.scala.Predef.String, _root_.scala.Predef.String] = _root_.scala.Predef.Map.empty): _root_.scala.concurrent.Future[CounterRemote]"
      ),
      s"missing create in:\n$content"
    )
    assert(
      content.contains("def getById(agentId: _root_.golem.AgentId): CounterRemote ="),
      s"missing getById in:\n$content"
    )
    removedConstructors.foreach { name =>
      assert(!content.contains(s"def $name("), s"removed constructor $name is generated:\n$content")
    }
    assert(!content.contains("java.util.UUID"), s"no constructor takes a raw UUID:\n$content")
    assert(!content.contains("generateIdempotencyKey"), s"no client derives an ID offline:\n$content")
  }

  test("constructors map to the upsert, create and get-by-id host functions") {
    val content = durableClient(params = List(ParamSurface("name", "String")))
    assert(
      content.contains("AgentClientRuntime.upsertWire[_root_.example.Counter, Id](agentType, name)"),
      s"upsert must call upsertWire:\n$content"
    )
    assert(content.contains(".createWire[_root_.example.Counter, Id]("), s"create must call createWire:\n$content")
    assert(
      content.contains("_root_.golem.runtime.rpc.CreateOptions(keyed, _root_.scala.Nil, env)"),
      s"create must pass keyed and env without config overrides:\n$content"
    )
    assert(
      content.contains("AgentClientRuntime.getByIdWire[_root_.example.Counter](agentType, agentId)"),
      s"getById must call getByIdWire:\n$content"
    )
    assert(
      content.contains("throw _root_.golem.runtime.rpc.AgentRpcException(err)"),
      s"upsert must unwrap the host result into the typed error:\n$content"
    )
    assert(!content.contains("AgentClientRuntime.resolve"), s"no removed resolve path:\n$content")
  }

  test("create takes typed config overrides and builds the ConfigOverride list internally") {
    val content = durableClient(
      params = List(ParamSurface("name", "String")),
      configFields = List(
        ConfigFieldSurface(List("appName"), "String"),
        ConfigFieldSurface(List("db", "host"), "String"),
        ConfigFieldSurface(List("db", "port"), "Int")
      )
    )
    assert(
      content.contains(
        "def create(name: String, keyed: _root_.scala.Boolean = false, appName: _root_.scala.Option[String] = _root_.scala.None, dbHost: _root_.scala.Option[String] = _root_.scala.None, dbPort: _root_.scala.Option[Int] = _root_.scala.None, env:"
      ),
      s"create must take the config fields after keyed:\n$content"
    )
    assert(content.contains("ConfigOverride[String](_root_.scala.List(\"db\", \"host\"), v)"), content)
    assert(content.contains("ConfigOverride[Int](_root_.scala.List(\"db\", \"port\"), v)"), content)
    assert(content.contains(").flatten, env)"), s"create must pass the overrides and env:\n$content")
    assert(content.contains("def upsert(name: String): CounterRemote"), s"upsert takes no overrides:\n$content")
    assert(!content.contains("configOverrides: _root_.scala.List"), s"no raw ConfigOverride list:\n$content")
  }

  test("ephemeral agent generates only upsert with metadata-bearing calls") {
    val content = durableClient(
      params = List(ParamSurface("name", "String"), ParamSurface("id", "Int")),
      configFields = List(ConfigFieldSurface(List("key"), "String")),
      mode = "ephemeral"
    )
    assert(content.contains("def upsert(name: String, id: Int): CounterRemote"), s"missing upsert in:\n$content")
    assert(!content.contains("def create("), s"ephemeral agent types cannot be created:\n$content")
    assert(!content.contains("def getById("), s"ephemeral agent should not have getById:\n$content")
    assert(!content.contains("def bindSelf()"), s"ephemeral agent should not have bindSelf:\n$content")
    removedConstructors.foreach { name =>
      assert(!content.contains(s"def $name("), s"removed constructor $name is generated:\n$content")
    }
    assert(content.contains("awaitWithMetadata"), s"await should return metadata:\n$content")
    assert(content.contains("triggerWithMetadata"), s"trigger should return metadata:\n$content")
  }

  test("durable agent with unit constructor generates no-param upsert") {
    val content = durableClient()
    assert(content.contains("def upsert(): CounterRemote"), s"missing no-param upsert in:\n$content")
    assert(content.contains("upsertWire[_root_.example.Counter, Id](agentType, ())"), content)
    assert(
      content.contains("def create(keyed: _root_.scala.Boolean = false, env:"),
      s"missing no-param create in:\n$content"
    )
  }

  test("remote clients expose the typed agent ID") {
    val content = durableClient()
    assert(content.contains("def agentId(): _root_.scala.concurrent.Future[_root_.golem.AgentId]\n"), content)
    assert(content.contains("resolved.agentId()"), content)
  }

  test("multi-param constructor packs args as Vector matching the runtime InputRecordCodec") {
    val result = RpcCodegen.generate(
      agents = List(
        agent(
          "example.EphAgent",
          "example",
          "EphAgent",
          params = List(ParamSurface("name", "String"), ParamSurface("id", "Int")),
          mode = "durable",
          methods = List(MethodSurface("hello", Nil, "Future[String]", Nil))
        )
      ),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    // The runtime constructor codec is `InputRecordCodec.fromParams` (Vector[Any]),
    // so the generated `Id` type and packed value must be Vector[Any], NOT a tuple.
    assert(
      content.contains("private type Id = _root_.scala.collection.immutable.Vector[_root_.scala.Any]"),
      s"multi-param Id type should be Vector[Any], not a tuple:\n$content"
    )
    assert(
      content.contains("agentType, _root_.scala.Vector[_root_.scala.Any](name, id)"),
      s"multi-param constructor should pack args as Vector[Any]:\n$content"
    )
    assert(
      !content.contains("agentType, (name, id)"),
      s"multi-param constructor must not pass a tuple:\n$content"
    )
  }

  test("durable mode-aware constructors bind self through bindSelfWire") {
    val content = durableClient(configFields = List(ConfigFieldSurface(List("key"), "String")))
    assert(content.contains("def bindSelf()"))
    assert(content.contains("AgentClientRuntime.bindSelfWire"))
    assert(
      !content.contains("AgentClientRuntime.upsert["),
      s"generated clients must avoid the reflective upsert:\n$content"
    )
  }

  test("no mode-aware constructors when no methods") {
    val result = RpcCodegen.generate(
      agents = List(agent("example.MyAgent", "example", "MyAgent", mode = "durable")),
      existingObjects = Seq.empty
    )

    val content = result.files.head.content
    List("upsert", "create", "getById").foreach { name =>
      assert(!content.contains(s"def $name("), s"should not have $name without methods:\n$content")
    }
  }
}
