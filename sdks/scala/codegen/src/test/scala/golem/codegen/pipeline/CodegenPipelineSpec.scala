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

package golem.codegen.pipeline

import golem.codegen.discovery.SourceDiscovery

class CodegenPipelineSpec extends munit.FunSuite {

  private val agentSource = SourceDiscovery.SourceInput(
    "Counter.scala",
    """|package example
       |
       |@agentDefinition("counter-agent")
       |trait CounterAgent {
       |  class Id(val value: String)
       |  def increment(amount: Int): Int
       |}
       |
       |@agentImplementation()
       |final class CounterAgentImpl(private val value: String) extends CounterAgent {
       |  def increment(amount: Int): Int = amount
       |}
       |""".stripMargin
  )

  private val toolSource = SourceDiscovery.SourceInput(
    "Grep.scala",
    """|package example
       |
       |import golem.runtime.annotations._
       |
       |@toolDefinition(version = "1.0.0")
       |trait Grep {
       |  def grep(pattern: String): String
       |}
       |
       |@toolImplementation()
       |final class GrepImpl extends Grep {
       |  def grep(pattern: String): String = pattern
       |}
       |""".stripMargin
  )

  private def discover(sources: SourceDiscovery.SourceInput*): SourceDiscovery.Result =
    SourceDiscovery.discover(sources)

  test("pipeline with both auto-register and rpc enabled") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, Some("example"), rpcEnabled = true)

    assert(result.autoRegister.isDefined)
    assert(result.autoRegister.get.files.nonEmpty)
    assert(result.autoRegister.get.implCount == 1)

    assert(result.rpc.files.nonEmpty)
    assertEquals(result.rpc.files.size, 1)
    assert(result.rpc.files.head.content.contains("CounterAgentClient"))
  }

  test("auto-register includes tool implementations") {
    val discovered = discover(toolSource)
    val result     = CodegenPipeline.run(discovered, Some("example"), rpcEnabled = false)

    assert(result.autoRegister.isDefined)
    val autoRegister = result.autoRegister.get
    assertEquals(autoRegister.implCount, 1)

    val content = autoRegister.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(content.contains("import golem.runtime.autowire.ToolImplementation"))
    assert(content.contains("ToolImplementation.registerClass[example.Grep, example.GrepImpl]"))
    assert(!content.contains("AgentImplementation.registerClass[Grep"))
  }

  test("auto-register can mix agent and tool implementations in one package") {
    val discovered = discover(agentSource, toolSource)
    val result     = CodegenPipeline.run(discovered, Some("example"), rpcEnabled = false)

    assert(result.autoRegister.isDefined)
    val autoRegister = result.autoRegister.get
    assertEquals(autoRegister.implCount, 2)
    assertEquals(autoRegister.packageCount, 1)

    val content = autoRegister.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(content.contains("AgentImplementation.registerClass[example.CounterAgent, example.CounterAgentImpl]"))
    assert(content.contains("ToolImplementation.registerClass[example.Grep, example.GrepImpl]"))
  }

  test("auto-register source changes when tool definition version changes") {
    def source(version: String) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition(version = "$version")
          |trait Grep {
          |  def grep(pattern: String): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: String): String = pattern
          |}
          |""".stripMargin
    )

    val originalAutoRegister = CodegenPipeline
      .run(discover(source("1.0.0")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val updatedAutoRegister = CodegenPipeline
      .run(discover(source("1.0.1")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(originalAutoRegister.contains("__golemSurfaceVersion"))
    assert(updatedAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(originalAutoRegister, updatedAutoRegister)
  }

  test("auto-register source changes when positional tool definition version changes") {
    def source(version: String) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition("grep", "$version")
          |trait Grep {
          |  def grep(pattern: String): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: String): String = pattern
          |}
          |""".stripMargin
    )

    val originalAutoRegister = CodegenPipeline
      .run(discover(source("1.0.0")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val updatedAutoRegister = CodegenPipeline
      .run(discover(source("1.0.1")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(originalAutoRegister.contains("__golemSurfaceVersion"))
    assert(updatedAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(originalAutoRegister, updatedAutoRegister)
  }

  test("auto-register source changes when tool argument short option changes") {
    def source(short: Char) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition(version = "1.0.0")
          |trait Grep {
          |  @arg("pattern", scope = "option", short = '$short')
          |  def grep(pattern: String): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: String): String = pattern
          |}
          |""".stripMargin
    )

    val originalAutoRegister = CodegenPipeline
      .run(discover(source('p')), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val updatedAutoRegister = CodegenPipeline
      .run(discover(source('q')), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(originalAutoRegister.contains("__golemSurfaceVersion"))
    assert(updatedAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(originalAutoRegister, updatedAutoRegister)
  }

  test("auto-register chooses tool trait when implementation has a base class") {
    val source = SourceDiscovery.SourceInput(
      "Grep.scala",
      """|package example
         |
         |import golem.runtime.annotations._
         |
         |abstract class ToolBase
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |
         |@toolImplementation()
         |final class GrepImpl extends ToolBase with Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(source), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.Grep, example.GrepImpl]"), content)
    assert(!content.contains("ToolImplementation.registerClass[ToolBase"), content)
  }

  test("auto-register ignores unrelated discovered tool traits when implementation has a same-named base class") {
    val unrelated = SourceDiscovery.SourceInput(
      "other/ToolBase.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait ToolBase {
         |  def unrelated(): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/Grep.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |
         |abstract class ToolBase
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |
         |@toolImplementation()
         |final class GrepImpl extends ToolBase with Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(unrelated, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.impl.Grep, example.impl.GrepImpl]"), content)
    assert(!content.contains("ToolImplementation.registerClass[other.ToolBase"), content)
  }

  test("auto-register source changes when tool result formatter metadata changes") {
    def source(defaultFormatter: String) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition(version = "1.0.0")
          |trait Grep {
          |  @result(formatters = Array("human", "json"), default = "$defaultFormatter")
          |  def grep(pattern: String): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: String): String = pattern
          |}
          |""".stripMargin
    )

    val humanAutoRegister = CodegenPipeline
      .run(discover(source("human")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val jsonAutoRegister = CodegenPipeline
      .run(discover(source("json")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(humanAutoRegister.contains("__golemSurfaceVersion"))
    assert(jsonAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(humanAutoRegister, jsonAutoRegister)
  }

  test("auto-register source changes when tool constraint metadata changes") {
    def source(requiredArg: String) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition(version = "1.0.0")
          |trait Grep {
          |  @arg("pattern", scope = "option")
          |  @arg("mode", scope = "option")
          |  @constraint(requiresAll = Array("$requiredArg"))
          |  def grep(pattern: Option[String], mode: Option[String]): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: Option[String], mode: Option[String]): String = pattern.orElse(mode).getOrElse("")
          |}
          |""".stripMargin
    )

    val patternAutoRegister = CodegenPipeline
      .run(discover(source("pattern")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val modeAutoRegister = CodegenPipeline
      .run(discover(source("mode")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(patternAutoRegister.contains("__golemSurfaceVersion"))
    assert(modeAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(patternAutoRegister, modeAutoRegister)
  }

  test("auto-register source changes when tool command annotations metadata changes") {
    def source(readOnly: Boolean) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition(version = "1.0.0")
          |trait Grep {
          |  @annotations(readOnly = $readOnly, destructive = ${!readOnly})
          |  def grep(pattern: String): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: String): String = pattern
          |}
          |""".stripMargin
    )

    val readOnlyAutoRegister = CodegenPipeline
      .run(discover(source(readOnly = true)), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val destructiveAutoRegister = CodegenPipeline
      .run(discover(source(readOnly = false)), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(readOnlyAutoRegister.contains("__golemSurfaceVersion"))
    assert(destructiveAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(readOnlyAutoRegister, destructiveAutoRegister)
  }

  test("auto-register source changes when tool method documentation changes") {
    def source(doc: String) = SourceDiscovery.SourceInput(
      "Grep.scala",
      s"""|package example
          |
          |import golem.runtime.annotations._
          |
          |@toolDefinition(version = "1.0.0")
          |trait Grep {
          |  /** $doc */
          |  def grep(pattern: String): String
          |}
          |
          |@toolImplementation()
          |final class GrepImpl extends Grep {
          |  def grep(pattern: String): String = pattern
          |}
          |""".stripMargin
    )

    val firstDocAutoRegister = CodegenPipeline
      .run(discover(source("Search for a regex pattern.")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val secondDocAutoRegister = CodegenPipeline
      .run(discover(source("Find matching lines in input files.")), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(firstDocAutoRegister.contains("__golemSurfaceVersion"))
    assert(secondDocAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(firstDocAutoRegister, secondDocAutoRegister)
  }

  test("auto-register source changes when subtree tool command metadata changes") {
    def sources(childCommand: String) = Seq(
      SourceDiscovery.SourceInput(
        "Root.scala",
        """|package example
           |
           |import golem.runtime.annotations._
           |
           |@toolDefinition(version = "1.0.0")
           |trait Root {
           |  def child(): Child
           |}
           |
           |@toolImplementation()
           |final class RootImpl extends Root {
           |  def child(): Child = ???
           |}
           |""".stripMargin
      ),
      SourceDiscovery.SourceInput(
        "Child.scala",
        s"""|package example
            |
            |import golem.runtime.annotations._
            |
            |@toolDefinition(version = "1.0.0")
            |trait Child {
            |  @command(name = "$childCommand")
            |  def run(pattern: String): String
            |}
            |""".stripMargin
      )
    )

    def generated(childCommand: String) = CodegenPipeline
      .run(discover(sources(childCommand)*), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assertNotEquals(generated("run"), generated("execute"))
  }

  test("auto-register source changes when separate tool error metadata changes") {
    def sources(exitCode: Int) = Seq(
      SourceDiscovery.SourceInput(
        "Grep.scala",
        """|package example
           |
           |import golem.runtime.annotations._
           |
           |@toolDefinition(version = "1.0.0")
           |trait Grep {
           |  def grep(pattern: String): Either[GrepError, String]
           |}
           |
           |@toolImplementation()
           |final class GrepImpl extends Grep {
           |  def grep(pattern: String): Either[GrepError, String] = Right(pattern)
           |}
           |""".stripMargin
      ),
      SourceDiscovery.SourceInput(
        "GrepError.scala",
        s"""|package example
            |
            |import golem.runtime.annotations._
            |
            |enum GrepError {
            |  @error(kind = "usage-error", exitCode = $exitCode)
            |  case BadPattern
            |}
            |""".stripMargin
      )
    )

    def generated(exitCode: Int) = CodegenPipeline
      .run(discover(sources(exitCode)*), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assertNotEquals(generated(64), generated(65))
  }

  test("auto-register resolves imported tool trait package") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api.Grep
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register preserves named import for external tool trait") {
    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import external.api.Grep
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[external.api.Grep, example.impl.GrepImpl]"), content)
    assert(!content.contains("ToolImplementation.registerClass[example.impl.Grep"), content)
  }

  test("auto-register resolves relative imported tool trait before root package collision") {
    val rootApi = SourceDiscovery.SourceInput(
      "root-api/Grep.scala",
      """|package api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def rootOnly(): String
         |}
         |""".stripMargin
    )

    val nestedApi = SourceDiscovery.SourceInput(
      "example/api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def nestedOnly(): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "example/impl/GrepImpl.scala",
      """|package example
         |package impl
         |
         |import golem.runtime.annotations._
         |import api.Grep
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def nestedOnly(): String = "ok"
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(rootApi, nestedApi, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"), content)
    assert(!content.contains("ToolImplementation.registerClass[api.Grep"), content)
  }

  test("auto-register resolves relatively imported tool trait package from nested package") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def other(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example
         |package impl
         |
         |import golem.runtime.annotations._
         |import api.Grep
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register resolves tool trait through an imported package qualifier") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def other(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api
         |
         |@toolImplementation()
         |final class GrepImpl extends api.Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register preserves imported package qualifier for external tool trait") {
    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import external.api
         |
         |@toolImplementation()
         |final class GrepImpl extends api.Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[external.api.Grep, example.impl.GrepImpl]"), content)
    assert(!content.contains("ToolImplementation.registerClass[api.Grep"), content)
  }

  test("auto-register resolves relative package-qualified tool trait parent") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def other(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example
         |package impl
         |
         |import golem.runtime.annotations._
         |
         |@toolImplementation()
         |final class GrepImpl extends api.Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .map(file => s"${file.relativePath}\n${file.content}")
      .mkString("\n---\n")

    assert(
      content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"),
      s"generated auto-register files:\n$content"
    )
  }

  test("auto-register resolves imported tool trait when another discovered trait has the same name") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api.Grep
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register keeps local tool trait precedence over an imported trait with the same name") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def api(pattern: String): String
         |}
         |""".stripMargin
    )

    val local = SourceDiscovery.SourceInput(
      "impl/Grep.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def local(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api.Grep
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def local(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, local, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.impl.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register resolves wildcard-imported tool trait when another discovered trait has the same name") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def grep(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def other(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api._
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def grep(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.api.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register honors wildcard import exclusions when resolving tool traits") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def api(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def other(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api.{Grep => _, _}
         |import example.other._
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def other(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.other.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register honors wildcard import renamed selectors when resolving tool traits") {
    val api = SourceDiscovery.SourceInput(
      "api/Grep.scala",
      """|package example.api
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def api(pattern: String): String
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/Grep.scala",
      """|package example.other
         |
         |import golem.runtime.annotations._
         |
         |@toolDefinition(version = "1.0.0")
         |trait Grep {
         |  def other(pattern: String): String
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/GrepImpl.scala",
      """|package example.impl
         |
         |import golem.runtime.annotations._
         |import example.api.{Grep => ApiGrep, _}
         |import example.other._
         |
         |@toolImplementation()
         |final class GrepImpl extends Grep {
         |  def other(pattern: String): String = pattern
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(content.contains("ToolImplementation.registerClass[example.other.Grep, example.impl.GrepImpl]"), content)
  }

  test("auto-register source changes when root-qualified tool definition version changes") {
    def source(version: String) = Seq(
      SourceDiscovery.SourceInput(
        "api/Grep.scala",
        s"""|package example.api
            |
            |import golem.runtime.annotations._
            |
            |@toolDefinition(version = "$version")
            |trait Grep {
            |  def grep(pattern: String): String
            |}
            |""".stripMargin
      ),
      SourceDiscovery.SourceInput(
        "impl/GrepImpl.scala",
        """|package example.impl
           |
           |import golem.runtime.annotations._
           |
           |@toolImplementation()
           |final class GrepImpl extends _root_.example.api.Grep {
           |  def grep(pattern: String): String = pattern
           |}
           |""".stripMargin
      )
    )

    val originalAutoRegister = CodegenPipeline
      .run(discover(source("1.0.0")*), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    val updatedAutoRegister = CodegenPipeline
      .run(discover(source("1.0.1")*), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(originalAutoRegister.contains("__golemSurfaceVersion"))
    assert(updatedAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(originalAutoRegister, updatedAutoRegister)
  }

  test("auto-register chooses agent trait when implementation has a base class") {
    val source = SourceDiscovery.SourceInput(
      "Agent.scala",
      """|package example
         |
         |abstract class AgentBase
         |
         |@agentDefinition()
         |trait CounterAgent {
         |  class Id()
         |  def increment(): Int
         |}
         |
         |@agentImplementation()
         |final class CounterAgentImpl extends AgentBase with CounterAgent {
         |  def increment(): Int = 1
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(source), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(
      content.contains("AgentImplementation.registerClass[example.CounterAgent, example.CounterAgentImpl]"),
      content
    )
    assert(!content.contains("AgentImplementation.registerClass[AgentBase"), content)
  }

  test("auto-register resolves wildcard-imported agent trait when another discovered trait has the same name") {
    val api = SourceDiscovery.SourceInput(
      "api/CounterAgent.scala",
      """|package example.api
         |
         |@agentDefinition()
         |trait CounterAgent {
         |  class Id()
         |  def increment(): Int
         |}
         |""".stripMargin
    )

    val other = SourceDiscovery.SourceInput(
      "other/CounterAgent.scala",
      """|package example.other
         |
         |@agentDefinition()
         |trait CounterAgent {
         |  class Id()
         |  def other(): Int
         |}
         |""".stripMargin
    )

    val impl = SourceDiscovery.SourceInput(
      "impl/CounterAgentImpl.scala",
      """|package example.impl
         |
         |import example.api._
         |
         |@agentImplementation()
         |final class CounterAgentImpl extends CounterAgent {
         |  def increment(): Int = 1
         |}
         |""".stripMargin
    )

    val result  = CodegenPipeline.run(discover(api, other, impl), Some("example"), rpcEnabled = false)
    val content = result.autoRegister.get.files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(
      content.contains("AgentImplementation.registerClass[example.api.CounterAgent, example.impl.CounterAgentImpl]"),
      content
    )
  }

  test("auto-register source changes when root-qualified agent definition changes") {
    def source(extraMethod: String) = Seq(
      SourceDiscovery.SourceInput(
        "api/CounterAgent.scala",
        s"""|package example.api
            |
            |@agentDefinition()
            |trait CounterAgent {
            |  class Id()
            |  def increment(): Int
            |$extraMethod
            |}
            |""".stripMargin
      ),
      SourceDiscovery.SourceInput(
        "impl/CounterAgentImpl.scala",
        """|package example.impl
           |
           |@agentImplementation()
           |final class CounterAgentImpl extends _root_.example.api.CounterAgent {
           |  def increment(): Int = 1
           |}
           |""".stripMargin
      )
    )

    val originalAutoRegister = CodegenPipeline
      .run(discover(source("")*), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    val updatedAutoRegister = CodegenPipeline
      .run(discover(source("  def decrement(): Int")*), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example_impl.scala"))
      .get
      .content

    assert(originalAutoRegister.contains("__golemSurfaceVersion"))
    assert(updatedAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(originalAutoRegister, updatedAutoRegister)
  }

  test("pipeline with only auto-register") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, Some("example"), rpcEnabled = false)

    assert(result.autoRegister.isDefined)
    assert(result.rpc.files.isEmpty)
  }

  test("pipeline with only rpc enabled") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = true)

    assert(result.autoRegister.isEmpty)
    assert(result.rpc.files.nonEmpty)
  }

  test("pipeline with nothing enabled") {
    val discovered = discover(agentSource)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = false)

    assert(result.autoRegister.isEmpty)
    assert(result.rpc.files.isEmpty)
  }

  test("pipeline converts discovery methods to IR with principalParams") {
    val source = SourceDiscovery.SourceInput(
      "Agent.scala",
      """|package example
         |
         |@agentDefinition()
         |trait MyAgent {
         |  class Id()
         |  def process(caller: Principal, data: String): String
         |}
         |""".stripMargin
    )

    val discovered = discover(source)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = true)

    assert(result.rpc.files.nonEmpty)
    val content = result.rpc.files.head.content
    // Principal param should be filtered out
    assert(content.contains("def apply(data: String)"), s"missing filtered apply in:\n$content")
    assert(!content.contains("caller: Principal"), s"principal param should be filtered:\n$content")
  }

  test("pipeline converts mode to IR") {
    val source = SourceDiscovery.SourceInput(
      "Eph.scala",
      """|package example
         |
         |@agentDefinition(mode = DurabilityMode.Ephemeral)
         |trait EphAgent {
         |  class Id()
         |  def hello(): String
         |}
         |""".stripMargin
    )

    val discovered = discover(source)
    val result     = CodegenPipeline.run(discovered, None, rpcEnabled = true)

    assert(result.rpc.files.nonEmpty)
    val content = result.rpc.files.head.content
    assert(content.contains("getPhantom"), s"ephemeral agent should have getPhantom:\n$content")
    assert(!content.contains("def get("), s"ephemeral agent should not have get:\n$content")
  }

  test("auto-register source changes when agent trait surface changes") {
    val original = SourceDiscovery.SourceInput(
      "Counter.scala",
      """|package example
         |
         |@agentDefinition()
         |trait CounterAgent {
         |  class Id(val value: String)
         |  def increment(): Int
         |}
         |
         |@agentImplementation()
         |final class CounterAgentImpl(private val value: String) extends CounterAgent {
         |  def increment(): Int = 1
         |}
         |""".stripMargin
    )

    val updated = SourceDiscovery.SourceInput(
      "Counter.scala",
      """|package example
         |
         |@agentDefinition()
         |trait CounterAgent {
         |  class Id(val value: String)
         |  def increment(): Int
         |  def recordMessageViaHttp(message: String): String
         |}
         |
         |@agentImplementation()
         |final class CounterAgentImpl(private val value: String) extends CounterAgent {
         |  def increment(): Int = 1
         |  def recordMessageViaHttp(message: String): String = message
         |}
         |""".stripMargin
    )

    val originalAutoRegister = CodegenPipeline
      .run(discover(original), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    val updatedAutoRegister = CodegenPipeline
      .run(discover(updated), Some("example"), rpcEnabled = false)
      .autoRegister
      .get
      .files
      .find(_.relativePath.endsWith("__GolemAutoRegister_example.scala"))
      .get
      .content

    assert(originalAutoRegister.contains("__golemSurfaceVersion"))
    assert(updatedAutoRegister.contains("__golemSurfaceVersion"))
    assertNotEquals(originalAutoRegister, updatedAutoRegister)
  }
}
