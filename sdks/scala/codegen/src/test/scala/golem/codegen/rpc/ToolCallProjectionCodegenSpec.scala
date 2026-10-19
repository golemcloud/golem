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

class ToolCallProjectionCodegenSpec extends munit.FunSuite {

  private val ir = {
    val discovered = SourceDiscovery.discover(
      Seq(SourceDiscovery.SourceInput("Tools.scala", ToolMiddlewareContractFixtures.toolDefinitions))
    )
    ToolProjectionIR.build(discovered.tools.toList)
  }

  test("generates exactly one shared projection for every tool root") {
    val files = ToolCallProjectionCodegen.generate(ir.tools).files

    assertEquals(
      files.map(_.relativePath),
      Seq(
        "example/middleware/BackendEchoCallProjection.scala",
        "example/middleware/PublicEchoCallProjection.scala",
        "example/middleware/PublicNestedCallProjection.scala"
      )
    )
    files.foreach(file => assert(dialects.Scala3(file.content).parse[Source].toOption.nonEmpty, file.content))
  }

  test("shared projection owns command paths codecs and backend result families") {
    val content = ToolCallProjectionCodegen
      .generate(ir.tools)
      .files
      .find(_.relativePath.endsWith("PublicEchoCallProjection.scala"))
      .get
      .content

    assert(content.contains("ToolDefinitionMacro.tryMetadata[_root_.example.middleware.PublicEcho]"), content)
    assert(content.contains("commandPath = _root_.scala.List(\"nested\", \"inspect\")"), content)
    assert(content.contains("ToolErrorSchemaDerivation.derive[_root_.example.middleware.PublicError]"), content)
    assert(content.contains("def __await_echo[B <: _root_.golem.tool.ToolCallBackend]"), content)
    assert(content.contains("backend.awaitNoStdout"), content)
    assert(content.contains("backend.startNoStdout"), content)
    assert(content.contains("backend.startOutputs"), content)
    assert(content.contains("ToolCallPreparation.prepareInput"), content)
    assert(content.contains("ToolCallPreparation.decodeConcreteValue"), content)
    assert(content.contains("ConcreteCodec.derived[_root_.java.lang.String]"), content)
  }

  test("shared projection selects no-output and all four output channel combinations") {
    val source =
      """|package example
         |import golem.runtime.annotations._
         |import golem.tool.ToolOutputStream
         |@toolDefinition(name = "shapes")
         |trait Shapes {
         |  def noStdout(): String
         |  def stdoutOnly(stdout: ToolOutputStream): Unit
         |  def valueStdout(stdout: ToolOutputStream): String
         |  @arg("stderr", channel = "stderr")
         |  def stderrOnly(stderr: ToolOutputStream): Unit
         |  @arg("stderr", channel = "stderr")
         |  def both(stdout: ToolOutputStream, stderr: ToolOutputStream): String
         |}
         |""".stripMargin
    val discovered = SourceDiscovery.discover(Seq(SourceDiscovery.SourceInput("Shapes.scala", source)))
    val projection = ToolProjectionIR.build(discovered.tools.toList)
    val content    = ToolCallProjectionCodegen.generate(projection.tools).files.head.content

    assert(content.contains("backend.StartedNoStdout[_root_.scala.Nothing, _root_.java.lang.String]"), content)
    assert(
      content.contains("backend.StartedOutputs[_root_.scala.Nothing, _root_.java.lang.String]"),
      content
    )
    assert(content.contains("backend.startNoStdout"), content)
    assert(content.contains("stdout = true, stderr = false"), content)
    assert(content.contains("stdout = false, stderr = true"), content)
    assert(content.contains("stdout = true, stderr = true"), content)
  }

  test("distinct error types whose names mangle identically retain distinct codecs") {
    val source =
      """|package example
         |import golem.runtime.annotations._
         |import scala.concurrent.Future
         |@toolDefinition(name = "colliding-errors")
         |trait CollidingErrors {
         |  def first(): Future[Either[Foo.Bar, String]]
         |  def second(): Future[Either[Foo_Bar, String]]
         |}
         |""".stripMargin
    val discovered = SourceDiscovery.discover(Seq(SourceDiscovery.SourceInput("CollidingErrors.scala", source)))
    val projection = ToolProjectionIR.build(discovered.tools.toList)
    val content    = ToolCallProjectionCodegen.generate(projection.tools).files.head.content

    assert(content.contains("ToolErrorSchemaDerivation.derive[Foo.Bar]"), content)
    assert(content.contains("ToolErrorSchemaDerivation.derive[Foo_Bar]"), content)
  }
}
