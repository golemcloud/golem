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

class ToolRpcCodegenSpec extends munit.FunSuite {

  private def discoverTools(sources: (String, String)*): SourceDiscovery.Result =
    SourceDiscovery.discover(sources.map { case (path, content) =>
      SourceDiscovery.SourceInput(path, content)
    })

  private val grepSource =
    """package example
      |
      |import golem.runtime.annotations._
      |import golem.tool.{ToolInputStream, ToolOutputStream}
      |import scala.concurrent.Future
      |
      |@toolDefinition(version = "1.0.0")
      |trait Grep {
      |  @arg("case-sensitive", scope = "global")
      |  @arg("color", aliases = Array("colour"))
      |  def grep(
      |    caseSensitive: Boolean,
      |    color: String,
      |    pattern: String,
      |    files: Seq[String],
      |    stdin: ToolInputStream,
      |    stdout: ToolOutputStream
      |  ): Either[GrepError, Long]
      |
      |  def replace(pattern: String, replacement: String): Future[Either[GrepError, Long]]
      |
      |  @arg("times", kind = "count-flag")
      |  def repeat(input: String, times: Int): String
      |
      |  def version(): String
      |}
      |""".stripMargin

  private val gitSource =
    """package example
      |
      |import golem.runtime.annotations._
      |import golem.Principal
      |
      |@toolDefinition
      |trait Git {
      |  @arg("git-dir", scope = "global")
      |  def git(gitDir: Option[String]): Unit
      |
      |  def status(short: Boolean, principal: Principal): Either[GitError, String]
      |
      |  @arg("verbose", kind = "flag")
      |  def remote(verbose: Boolean): Remote
      |}
      |
      |@toolDefinition
      |trait Remote {
      |  def add(name: String, url: String): Either[GitError, Unit]
      |
      |  def remote(): Seq[String]
      |}
      |""".stripMargin

  private def generate(sources: (String, String)*): ToolRpcCodegen.Result = {
    val discovered = discoverTools(sources: _*)
    ToolRpcCodegen.generate(discovered.tools.toList, discovered.objects)
  }

  test("discovers tool traits with command/arg annotations") {
    val discovered = discoverTools("Grep.scala" -> grepSource)
    assertEquals(discovered.tools.size, 1)
    val tool = discovered.tools.head
    assertEquals(tool.name, "Grep")
    assertEquals(tool.pkg, "example")
    assertEquals(tool.toolName, None)
    val grep = tool.methods.find(_.name == "grep").get
    assertEquals(grep.args.map(_.name).toSet, Set("case-sensitive", "color"))
    assertEquals(grep.args.find(_.name == "case-sensitive").get.scope, Some("global"))
    assertEquals(grep.args.find(_.name == "color").get.aliases, List("colour"))
    val repeat = tool.methods.find(_.name == "repeat").get
    assertEquals(repeat.args.find(_.name == "times").get.kind, Some("count-flag"))
  }

  test("generates a root client trait and companion for a leaf-only tool") {
    val result = generate("Grep.scala" -> grepSource)
    assertEquals(result.files.map(_.relativePath), Seq("example/GrepClient.scala"))
    val content = result.files.head.content

    assert(content.contains("package example"))
    assert(content.contains("trait GrepClient {"))
    assert(content.contains("object GrepClient {"))
    assert(content.contains("""val toolName: _root_.scala.Predef.String = "grep""""))
    assert(content.contains("def apply(): GrepClient = new Root()"))
    assert(content.contains("_root_.golem.runtime.macros.ToolDefinitionMacro.tryMetadata[Grep]"))
  }

  test("drops Principal and stdout parameters and keeps stdin; stdout moves to the result") {
    val content = generate("Grep.scala" -> grepSource).files.head.content

    // stdout excluded from the signature but present in the result tuple
    assert(
      content.contains(
        "def grep(caseSensitive: Boolean, color: String, pattern: String, files: Seq[String], " +
          "stdin: _root_.golem.tool.ToolInputStream): " +
          "_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[GrepError], " +
          "(Long, _root_.golem.tool.ToolOutputStream)]]"
      )
    )
    assert(content.contains("_root_.scala.Some(stdin)"))
    assert(content.contains("decodeValueStdoutResult"))
  }

  test("the implicit-body root command invokes with an empty command path") {
    val content = generate("Grep.scala" -> grepSource).files.head.content
    // `grep` is the tool's root command: no path element is appended
    assert(
      content.contains("_root_.golem.tool.ToolClientRuntime.run[GrepError](__transport, _root_.scala.Nil, __input")
    )
  }

  test("unwraps Future results and decodes typed errors through the derived error schema") {
    val content = generate("Grep.scala" -> grepSource).files.head.content
    assert(content.contains("private lazy val __errorSchema_GrepError: _root_.golem.tool.ToolErrorSchema[GrepError]"))
    assert(content.contains("_root_.golem.runtime.macros.ToolErrorSchemaDerivation.derive[GrepError]"))
    assert(content.contains("__errorSchema_GrepError.fromErrorPayloadValue(_)"))
    // subcommands inherit the root global `caseSensitive`
    assert(
      content.contains(
        "def replace(caseSensitive: Boolean, pattern: String, replacement: String): " +
          "_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[GrepError], Long]]"
      )
    )
  }

  test("methods without a declared error type generate infallible calls") {
    val content = generate("Grep.scala" -> grepSource).files.head.content
    assert(
      content.contains(
        "def version(caseSensitive: Boolean): " +
          "_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[_root_.scala.Nothing], String]]"
      )
    )
    assert(content.contains("runInfallible"))
  }

  test("count-flag parameters encode through countFlagValue") {
    val content = generate("Grep.scala" -> grepSource).files.head.content
    assert(content.contains("""("times", _root_.golem.tool.ToolClientRuntime.countFlagValue(times))"""))
  }

  test("subcommands inherit root globals and use canonical field names") {
    val result  = generate("Git.scala" -> gitSource)
    val content = result.files.find(_.relativePath == "example/GitClient.scala").get.content

    // status inherits the root global `gitDir` and excludes the Principal parameter
    assert(
      content.contains(
        "def status(gitDir: Option[String], short: Boolean): " +
          "_root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[GitError], String]]"
      )
    )
    assert(
      content.contains(
        """("git-dir", _root_.scala.Predef.implicitly[_root_.golem.schema.IntoSchema[Option[String]]].toValue(gitDir))"""
      )
    )
    assert(
      content.contains(
        """_root_.golem.tool.ToolClientRuntime.run[GitError](__transport, _root_.scala.List("status"), __input"""
      )
    )
    assert(content.contains("""private lazy val __model_status"""))
  }

  test("subtree methods return wrapper clients carrying the inherited canonical prefix") {
    val result  = generate("Git.scala" -> gitSource)
    val content = result.files.find(_.relativePath == "example/GitClient.scala").get.content

    assert(content.contains("def remote(gitDir: Option[String], verbose: Boolean): GitClient.RemoteClient"))
    assert(content.contains("final class RemoteClient private[GitClient] ("))
    // prefix packs the inherited global then the subtree method's own flag
    val prefixIdx  = content.indexOf("""prefixValue("git-dir"""")
    val verboseIdx = content.indexOf("""prefixValue("verbose"""")
    assert(prefixIdx >= 0 && verboseIdx >= 0 && prefixIdx < verboseIdx)
    // navigation appends the child command name and creates a fresh transport
    assert(content.contains("""_root_.scala.List("remote")"""))
    assert(content.contains("_root_.golem.runtime.tool.client.ToolRpcClient.transport(GitClient.toolName)"))
  }

  test("wrapper leaf methods use the dynamic input path when a prefix is inherited") {
    val result  = generate("Git.scala" -> gitSource)
    val content = result.files.find(_.relativePath == "example/GitClient.scala").get.content

    assert(content.contains("def add(name: String, url: String):"))
    assert(content.contains("if (__inheritedPrefix.isEmpty)"))
    assert(content.contains("buildDynamicInput"))
    assert(content.contains("""__commandPath :+ "add""""))
    // the child's implicit-body command does not extend the command path
    assert(content.contains("runInfallible(__transport, __commandPath, __input"))
    // the child descriptor backs the wrapper's canonical input models
    assert(content.contains("ToolDefinitionMacro.tryMetadata[Remote]"))
  }

  test("child wrappers omit parameters supplied through inherited canonical aliases") {
    val source =
      """package example
        |
        |import golem.runtime.annotations._
        |
        |@toolDefinition(name = "root")
        |trait RootTool {
        |  @arg("config", aliases = Array("cfg"), scope = "global")
        |  def root(config: String): Unit
        |
        |  @arg("config")
        |  def group(config: String): ChildTool
        |}
        |
        |@toolDefinition(name = "child")
        |trait ChildTool {
        |  @arg("cfg")
        |  def child(cfg: String): Unit
        |}
        |""".stripMargin

    val result  = generate("RootTool.scala" -> source)
    val content = result.files.find(_.relativePath == "example/RootToolClient.scala").get.content

    assert(content.contains("def group(config: String): RootToolClient.GroupClient"))
    assert(content.contains("""prefixValue("config", _root_.scala.List("cfg"), config"""))
    assert(content.contains("def child():"))
    assert(!content.contains("def child(cfg: String):"))
    assert(!content.contains("""("cfg", _root_.scala.Predef.implicitly"""))
  }

  test("child wrappers do not omit subtree local aliases that were not emitted into the prefix") {
    val source =
      """package example
        |
        |import golem.runtime.annotations._
        |
        |@toolDefinition(name = "root")
        |trait RootTool {
        |  @arg("config", aliases = Array("cfg"), scope = "global")
        |  def root(config: String): Unit
        |
        |  @arg("config", aliases = Array("local-cfg"))
        |  def group(config: String): ChildTool
        |}
        |
        |@toolDefinition(name = "child")
        |trait ChildTool {
        |  @arg("local-cfg")
        |  def child(localCfg: String): Unit
        |}
        |""".stripMargin

    val result  = generate("RootTool.scala" -> source)
    val content = result.files.find(_.relativePath == "example/RootToolClient.scala").get.content

    assert(content.contains("def group(config: String): RootToolClient.GroupClient"))
    assert(content.contains("""prefixValue("config", _root_.scala.List("cfg"), config"""))
    assert(
      !content.contains("""prefixValue("config", _root_.scala.List("cfg", "local-cfg"), config"""),
      "the inherited prefix should not advertise the subtree-local alias"
    )
    assert(content.contains("def child(localCfg: String):"))
    assert(content.contains("""("local-cfg", _root_.scala.Predef.implicitly"""))
    assert(!content.contains("def child():"))
  }

  test("grandchild wrappers do not omit aliases from a child root global that was not added to the prefix") {
    val source =
      """package example
        |
        |import golem.runtime.annotations._
        |
        |@toolDefinition(name = "root")
        |trait RootTool {
        |  @arg("config", aliases = Array("root-cfg"), scope = "global")
        |  def root(config: String): Unit
        |
        |  def group(): ChildTool
        |}
        |
        |@toolDefinition(name = "child")
        |trait ChildTool {
        |  @arg("config", aliases = Array("child-cfg"), scope = "global")
        |  def child(config: String): Unit
        |
        |  @arg("config")
        |  def nested(config: String): GrandChildTool
        |}
        |
        |@toolDefinition(name = "grand-child")
        |trait GrandChildTool {
        |  @arg("child-cfg")
        |  def grand(childCfg: String): Unit
        |}
        |""".stripMargin

    val result  = generate("RootTool.scala" -> source)
    val content = result.files.find(_.relativePath == "example/RootToolClient.scala").get.content

    assert(content.contains("""prefixValue("config", _root_.scala.List("root-cfg"), config"""))
    assert(content.contains("def nested(): RootToolClient.GroupNestedClient"))
    assert(content.contains("def grand(childCfg: String):"))
    assert(content.contains("""("child-cfg", _root_.scala.Predef.implicitly"""))
    assert(!content.contains("def grand():"))
  }

  test("grandchild wrappers keep child root-global aliases when the child global was already inherited by another surface") {
    val source =
      """package example
        |
        |import golem.runtime.annotations._
        |
        |@toolDefinition(name = "root")
        |trait RootTool {
        |  @arg("config", aliases = Array("root-cfg"), scope = "global")
        |  def root(config: String): Unit
        |
        |  def group(): ChildTool
        |}
        |
        |@toolDefinition(name = "child")
        |trait ChildTool {
        |  @arg("config", aliases = Array("child-cfg"), scope = "global")
        |  def child(config: String): Unit
        |
        |  def nested(): GrandChildTool
        |}
        |
        |@toolDefinition(name = "grand-child")
        |trait GrandChildTool {
        |  @arg("child-cfg")
        |  def grand(childCfg: String): Unit
        |}
        |""".stripMargin

    val result  = generate("RootTool.scala" -> source)
    val content = result.files.find(_.relativePath == "example/RootToolClient.scala").get.content

    assert(content.contains("""prefixValue("config", _root_.scala.List("root-cfg"), config"""))
    assert(content.contains("def nested(): RootToolClient.GroupNestedClient"))
    assert(content.contains("def grand(childCfg: String):"))
    assert(content.contains("""("child-cfg", _root_.scala.Predef.implicitly"""))
    assert(!content.contains("def grand():"))
  }

  test("every tool trait also gets its own standalone root client") {
    val result = generate("Git.scala" -> gitSource)
    assert(result.files.exists(_.relativePath == "example/RemoteClient.scala"))
    val content = result.files.find(_.relativePath == "example/RemoteClient.scala").get.content
    assert(content.contains("trait RemoteClient {"))
    assert(content.contains("""val toolName: _root_.scala.Predef.String = "remote""""))
  }

  test("skips generation when the client object already exists") {
    val discovered = discoverTools("Grep.scala" -> grepSource)
    val result     = ToolRpcCodegen.generate(
      discovered.tools.toList,
      Seq(SourceDiscovery.ExistingObject("GrepClient.scala", "example", "GrepClient"))
    )
    assertEquals(result.files.size, 0)
    assertEquals(result.warnings.size, 1)
    assert(result.warnings.head.message.contains("already exists"))
  }

  test("cuts subtree cycles with a warning") {
    val cyclic =
      """package example
        |
        |import golem.runtime.annotations._
        |
        |@toolDefinition
        |trait Ping {
        |  def ping(): String
        |  def pong(): Pong
        |}
        |
        |@toolDefinition
        |trait Pong {
        |  def pong(): String
        |  def ping(): Ping
        |}
        |""".stripMargin

    val result = generate("Cyclic.scala" -> cyclic)
    assert(result.warnings.exists(_.message.contains("subtree cycle")))
    assert(result.files.exists(_.relativePath == "example/PingClient.scala"))
    assert(result.files.exists(_.relativePath == "example/PongClient.scala"))
  }

  test("honors the tool name override from @toolDefinition") {
    val source =
      """package example
        |
        |import golem.runtime.annotations._
        |
        |@toolDefinition(name = "super-grep")
        |trait Grep2 {
        |  def superGrep(pattern: String): String
        |}
        |""".stripMargin

    val content = generate("Grep2.scala" -> source).files.head.content
    assert(content.contains("""val toolName: _root_.scala.Predef.String = "super-grep""""))
    // the overridden root command is the implicit body: empty command path
    assert(content.contains("runInfallible(__transport, _root_.scala.Nil, __input"))
  }
}
