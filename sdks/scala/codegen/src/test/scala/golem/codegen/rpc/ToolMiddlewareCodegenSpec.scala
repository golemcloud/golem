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
import golem.codegen.fixtures.ToolMiddlewareContractFixtures

import scala.meta._
import scala.meta.parsers._

class ToolMiddlewareCodegenSpec extends munit.FunSuite {

  private def projection(source: String): ToolProjectionIR.Result = {
    val discovered = SourceDiscovery.discover(Seq(SourceDiscovery.SourceInput("Tools.scala", source)))
    ToolProjectionIR.build(discovered.tools.toList)
  }

  private def projection(sources: Seq[(String, String)]): ToolProjectionIR.Result = {
    val discovered = SourceDiscovery.discover(sources.map { case (path, source) =>
      SourceDiscovery.SourceInput(path, source)
    })
    ToolProjectionIR.build(discovered.tools.toList)
  }

  test("shared projection flattens nested leaves with canonical parameters and exact shapes") {
    val result = projection(ToolMiddlewareContractFixtures.toolDefinitions)
    assertEquals(result.errors, Nil)

    val public = result.tools.find(_.name == "PublicEcho").get
    assertEquals(public.flattenedLeaves.map(_.name), List("publicEcho", "echo", "copy", "inspect"))

    val echo = public.flattenedLeaves.find(_.name == "echo").get
    assertEquals(echo.commandPath, List("echo"))
    assertEquals(echo.params.map(_.param.ident), List("config", "value", "principal"))
    assertEquals(echo.underlyingParams.map(_.param.ident), List("config", "value"))
    assertEquals(echo.codec.okType, Some("String"))
    assertEquals(echo.codec.errType, Some("PublicError"))
    assertEquals(echo.codec.projectedOkType, Some("_root_.java.lang.String"))
    assertEquals(echo.codec.projectedErrType, Some("_root_.example.middleware.PublicError"))

    val inspect = public.flattenedLeaves.find(_.name == "inspect").get
    assertEquals(inspect.commandPath, List("nested", "inspect"))
    assertEquals(inspect.params.map(_.param.ident), List("config", "prefix", "name"))
    assertEquals(inspect.params.map(_.canonicalName), List("config", "prefix", "name"))
  }

  test("generates nominal underlying and transparent/adapter middleware surfaces") {
    val ir     = projection(ToolMiddlewareContractFixtures.toolDefinitions)
    val result = ToolMiddlewareCodegen.generate(ir.tools, Nil)

    assertEquals(result.errors, Nil)
    assertEquals(
      result.files.map(_.relativePath),
      Seq(
        "example/middleware/BackendEchoMiddleware.scala",
        "example/middleware/PublicEchoMiddleware.scala",
        "example/middleware/PublicNestedMiddleware.scala"
      )
    )

    val content = result.files.find(_.relativePath.endsWith("PublicEchoMiddleware.scala")).get.content
    assert(dialects.Scala3(content).parse[Source].toOption.nonEmpty, content)
    assert(content.contains("trait PublicEchoUnderlying"))
    assert(content.contains("trait PublicEchoMiddleware extends PublicEchoMiddleware.Adapter[PublicEchoUnderlying]"))
    assert(content.contains("trait Adapter[U]"))
    assert(
      content.contains(
        "def echo(underlying: U, @_root_.golem.runtime.annotations.internalToolMiddlewareField(\"config\", false) config: _root_.java.lang.String, " +
          "@_root_.golem.runtime.annotations.internalToolMiddlewareField(\"value\", false) value: _root_.java.lang.String, principal: _root_.golem.Principal): " +
          "_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolInvokeError[_root_.example.middleware.PublicError], _root_.java.lang.String]]"
      ),
      content
    )
    assert(
      content.contains(
        "def inspect(@_root_.golem.runtime.annotations.internalToolMiddlewareField(\"config\", false) config: _root_.java.lang.String, " +
          "@_root_.golem.runtime.annotations.internalToolMiddlewareField(\"prefix\", false) prefix: _root_.java.lang.String, " +
          "@_root_.golem.runtime.annotations.internalToolMiddlewareField(\"name\", false) name: _root_.java.lang.String): " +
          "_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolInvokeError[_root_.scala.Nothing], _root_.java.lang.String]]"
      ),
      content
    )
    assert(
      content.contains(
        "ToolUnderlyingRuntime.staticInputModel(__descriptor, _root_.scala.List(\"nested\", \"inspect\"))"
      )
    )
    assert(content.contains("def __golemFromRaw(underlying: _root_.golem.tool.RawToolUnderlying)"))
  }

  test("projection qualifies Scala default aliases with valid root paths") {
    val result = projection(
      """|package example
         |import golem.runtime.annotations._
         |@toolDefinition(name = "collections")
         |trait Collections {
         |  def transform(values: Map[String, Set[String]]): Map[String, Set[String]]
         |}
         |""".stripMargin
    )

    assertEquals(result.errors, Nil)
    val transform = result.tools.head.flattenedLeaves.head
    val mapType   =
      "_root_.scala.collection.immutable.Map[_root_.java.lang.String, _root_.scala.collection.immutable.Set[_root_.java.lang.String]]"
    assertEquals(transform.params.head.param.projectedTypeExpr, mapType)
    assertEquals(transform.codec.projectedOkType, Some(mapType))
  }

  test("projection resolves renamed imports and qualifies cross-package flattened types") {
    val result = projection(
      Seq(
        "Domain.scala" ->
          """|package domain
             |import golem.runtime.annotations._
             |final case class Input(value: String)
             |sealed trait ChildError
             |@toolDefinition(name = "child")
             |trait Child {
             |  def call(input: Input): Either[ChildError, Input]
             |}
             |""".stripMargin,
        "Api.scala" ->
          """|package api
             |import domain.{Child => Nested}
             |import golem.{Principal => Caller}
             |import golem.runtime.annotations._
             |@toolDefinition(name = "root")
             |trait Root {
             |  def nested(caller: Caller): Nested
             |}
             |""".stripMargin
      )
    )

    assertEquals(result.errors, Nil)
    val root = result.tools.find(_.fqn == "api.Root").get
    val call = root.flattenedLeaves.find(_.name == "call").get
    assertEquals(call.commandPath, List("nested", "call"))
    assertEquals(call.params.map(_.param.ident), List("input"))
    assertEquals(call.codec.projectedOkType, Some("_root_.domain.Input"))
    assertEquals(call.codec.projectedErrType, Some("_root_.domain.ChildError"))
  }

  test("projection resolves wildcard subtree imports and does not treat a local Principal as injected") {
    val result = projection(
      Seq(
        "Domain.scala" ->
          """|package domain
             |import golem.runtime.annotations._
             |@toolDefinition(name = "child")
             |trait Child { def call(): String }
             |""".stripMargin,
        "Api.scala" ->
          """|package api
             |import domain._
             |import golem.runtime.annotations._
             |final case class Principal(value: String)
             |@toolDefinition(name = "root")
             |trait Root {
             |  def nested(principal: Principal): Child
             |}
             |""".stripMargin
      )
    )

    assertEquals(result.errors, Nil)
    val call = result.tools.find(_.fqn == "api.Root").get.flattenedLeaves.find(_.name == "call").get
    assertEquals(call.params.map(_.param.ident), List("principal"))
    assertEquals(call.params.head.param.projectedTypeExpr, "_root_.api.Principal")
  }

  test("projection resolves relative imports through enclosing packages") {
    val result = projection(
      Seq(
        "Child.scala" ->
          """|package example.api
             |import golem.runtime.annotations._
             |final case class Payload(value: String)
             |sealed trait ChildError
             |@toolDefinition(name = "child")
             |trait Child {
             |  def call(payload: Payload): Either[ChildError, Payload]
             |}
             |""".stripMargin,
        "Root.scala" ->
          """|package example.impl
             |import api.Child
             |import golem.runtime.annotations._
             |@toolDefinition(name = "root")
             |trait Root {
             |  def child(): Child
             |}
             |""".stripMargin
      )
    )

    assertEquals(result.errors, Nil)
    val call = result.tools.find(_.fqn == "example.impl.Root").get.flattenedLeaves.find(_.name == "call").get
    assertEquals(call.commandPath, List("child", "call"))
    assertEquals(call.params.head.param.projectedTypeExpr, "_root_.example.api.Payload")
    assertEquals(call.codec.projectedOkType, Some("_root_.example.api.Payload"))
    assertEquals(call.codec.projectedErrType, Some("_root_.example.api.ChildError"))
  }

  test("projection preserves unresolved wildcard import scope without inventing a root type") {
    val ir = projection(
      """|package example
         |import external.schema._
         |import golem.runtime.annotations._
         |@toolDefinition(name = "external")
         |trait External {
         |  def transform(value: ExternalValue): ExternalValue
         |}
         |""".stripMargin
    )

    assertEquals(ir.errors, Nil)
    val transform = ir.tools.head.flattenedLeaves.head
    assertEquals(transform.params.head.param.projectedTypeExpr, "ExternalValue")
    assertEquals(transform.codec.projectedOkType, Some("ExternalValue"))

    val generated = ToolMiddlewareCodegen.generate(ir.tools, Nil).files.head.content
    assert(generated.contains("import external.schema._"), generated)
    assert(!generated.contains("_root_.ExternalValue"), generated)

    val client = ToolRpcCodegen.generateFromIR(ir.tools, Nil).files.head.content
    assert(client.contains("import external.schema._"), client)
    assert(!client.contains("_root_.ExternalValue"), client)
  }

  test("parent projections inherit unresolved wildcard imports from flattened children") {
    val ir = projection(
      Seq(
        "Child.scala" ->
          """|package nested.child
             |import external.schema._
             |import golem.runtime.annotations._
             |@toolDefinition(name = "child")
             |trait Child {
             |  def transform(value: ExternalValue): ExternalValue
             |}
             |""".stripMargin,
        "Root.scala" ->
          """|package nested.root
             |import golem.runtime.annotations._
             |import nested.child.Child
             |@toolDefinition(name = "root")
             |trait Root {
             |  def child(): Child
             |}
             |""".stripMargin
      )
    )

    assertEquals(ir.errors, Nil)
    val middleware = ToolMiddlewareCodegen
      .generate(ir.tools, Nil)
      .files
      .find(_.relativePath == "nested/root/RootMiddleware.scala")
      .get
      .content
    assert(middleware.contains("import external.schema._"), middleware)

    val client = ToolRpcCodegen
      .generateFromIR(ir.tools, Nil)
      .files
      .find(_.relativePath == "nested/root/RootClient.scala")
      .get
      .content
    assert(client.contains("import external.schema._"), client)
  }

  test("parent projections inherit unresolved imports from intermediate subtree parameters") {
    val ir = projection(
      Seq(
        "Leaf.scala" ->
          """|package deep.leaf
             |import golem.runtime.annotations._
             |@toolDefinition(name = "leaf")
             |trait Leaf { def run(): String }
             |""".stripMargin,
        "Mid.scala" ->
          """|package deep.mid
             |import deep.leaf.Leaf
             |import external.schema._
             |import golem.runtime.annotations._
             |@toolDefinition(name = "mid")
             |trait Mid { def leaf(value: ExternalValue): Leaf }
             |""".stripMargin,
        "Root.scala" ->
          """|package deep.root
             |import deep.mid.Mid
             |import golem.runtime.annotations._
             |@toolDefinition(name = "root")
             |trait Root { def mid(): Mid }
             |""".stripMargin
      )
    )

    assertEquals(ir.errors, Nil)
    val middleware = ToolMiddlewareCodegen
      .generate(ir.tools, Nil)
      .files
      .find(_.relativePath == "deep/root/RootMiddleware.scala")
      .get
      .content
    assert(middleware.contains("import external.schema._"), middleware)

    val client = ToolRpcCodegen
      .generateFromIR(ir.tools, Nil)
      .files
      .find(_.relativePath == "deep/root/RootClient.scala")
      .get
      .content
    assert(client.contains("import external.schema._"), client)
  }

  test("rejects flattened method collisions with both command paths") {
    val result = projection(ToolMiddlewareContractFixtures.invalidMiddlewareSources("flattened-collision"))
    val error  = result.errors.find(_.message.contains("flattened method `run`")).getOrElse(fail(result.toString))

    assert(error.message.contains("first run"), error.message)
    assert(error.message.contains("second run"), error.message)
  }

  test("rejects generated companion object collisions") {
    val ir     = projection(ToolMiddlewareContractFixtures.toolDefinitions)
    val result = ToolMiddlewareCodegen.generate(
      ir.tools,
      Seq(SourceDiscovery.ExistingObject("Existing.scala", "example.middleware", "PublicEchoMiddleware"))
    )

    assert(result.errors.exists(_.message.contains("PublicEchoMiddleware")))
    assert(!result.files.exists(_.relativePath.endsWith("PublicEchoMiddleware.scala")))
  }
}
