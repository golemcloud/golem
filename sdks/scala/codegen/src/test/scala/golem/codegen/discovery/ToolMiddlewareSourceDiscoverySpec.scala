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

package golem.codegen.discovery

import golem.codegen.fixtures.ToolMiddlewareContractFixtures
import golem.codegen.pipeline.CodegenPipeline

class ToolMiddlewareSourceDiscoverySpec extends munit.FunSuite {

  private def source(path: String, content: String): SourceDiscovery.SourceInput =
    SourceDiscovery.SourceInput(path, content)

  private def discover(sources: (String, String)*): SourceDiscovery.Result =
    SourceDiscovery.discover(sources.map { case (path, content) => source(path, content) })

  test("discovers transparent, adapter, and universal middleware contracts") {
    val result = discover(
      "Tools.scala"       -> ToolMiddlewareContractFixtures.toolDefinitions,
      "Transparent.scala" -> ToolMiddlewareContractFixtures.transparentMiddleware,
      "Adapter.scala"     -> ToolMiddlewareContractFixtures.adapterMiddleware,
      "Universal.scala"   -> ToolMiddlewareContractFixtures.universalMiddleware
    )

    assertEquals(result.errors, Nil)
    assertEquals(result.toolMiddlewares.map(_.middlewareName), Seq("echo-policy", "public-to-backend"))

    val transparent = result.toolMiddlewares.head
    assertEquals(transparent.implClass, "EchoPolicy")
    assertEquals(transparent.aliases, List("policy"))
    assertEquals(transparent.description, Some("Validates and forwards public echo calls"))
    assertEquals(transparent.presentedToolType, "example.middleware.PublicEcho")
    assertEquals(transparent.expectedToolType, "example.middleware.PublicEcho")
    assert(transparent.transparent)
    assertEquals(transparent.sourceHash.length, 64)
    assertEquals(transparent.surfaceHash.length, 64)

    val adapter = result.toolMiddlewares(1)
    assertEquals(adapter.implClass, "PublicToBackend")
    assertEquals(adapter.presentedToolType, "example.middleware.PublicEcho")
    assertEquals(adapter.expectedToolType, "example.middleware.BackendEcho")
    assert(!adapter.transparent)

    assertEquals(result.universalToolMiddlewares.size, 1)
    val universal = result.universalToolMiddlewares.head
    assertEquals(universal.middlewareName, "audit-all-tools")
    assertEquals(universal.implClass, "AuditAllTools")
    assertEquals(universal.description, Some("Audits and forwards every tool invocation"))
    assertEquals(universal.parentType, "UniversalToolMiddleware")
  }

  test("resolves renamed, wildcard, and root-qualified generated middleware parents") {
    val tools =
      """|package example.api
         |@toolDefinition(name = "presented")
         |trait Presented
         |@toolDefinition(name = "expected")
         |trait Expected
         |""".stripMargin
    val renamed =
      """|package example.renamed
         |import example.api.{PresentedMiddleware => PM, ExpectedUnderlying => EU}
         |@toolMiddleware(name = "renamed")
         |final class Renamed extends PM.Adapter[EU]
         |""".stripMargin
    val wildcard =
      """|package example.wildcard
         |import example.api._
         |@toolMiddleware(name = "wildcard")
         |final class Wildcard extends PresentedMiddleware.Adapter[ExpectedUnderlying]
         |""".stripMargin
    val rooted =
      """|package example.rooted
         |@toolMiddleware(name = "rooted")
         |final class Rooted
         |    extends _root_.example.api.PresentedMiddleware.Adapter[
         |      _root_.example.api.ExpectedUnderlying
         |    ]
         |""".stripMargin

    val result = discover(
      "api/Tools.scala"         -> tools,
      "renamed/Renamed.scala"   -> renamed,
      "wildcard/Wildcard.scala" -> wildcard,
      "rooted/Rooted.scala"     -> rooted
    )

    assertEquals(result.errors, Nil)
    result.toolMiddlewares.foreach { middleware =>
      assertEquals(middleware.presentedToolType, "example.api.Presented")
      assertEquals(middleware.expectedToolType, "example.api.Expected")
    }
  }

  test("root-qualified generated parents ignore colliding renamed imports") {
    val tools =
      """|package example.api
         |@toolDefinition(name = "presented")
         |trait Presented
         |@toolDefinition(name = "expected")
         |trait Expected
         |""".stripMargin
    val middleware =
      """|package example.impl
         |import wrong.{alias => example}
         |@toolMiddleware(name = "rooted")
         |final class Rooted
         |    extends _root_.example.api.PresentedMiddleware.Adapter[
         |      _root_.example.api.ExpectedUnderlying
         |    ]
         |""".stripMargin

    val result = discover("Tools.scala" -> tools, "Rooted.scala" -> middleware)
    assertEquals(result.errors, Nil)
    assertEquals(result.toolMiddlewares.head.presentedToolType, "example.api.Presented")
    assertEquals(result.toolMiddlewares.head.expectedToolType, "example.api.Expected")
  }

  test("rejects unproven explicit, named-imported, and wildcard-imported tool projections") {
    val tools =
      """|package example.api
         |@toolDefinition(name = "presented")
         |trait Presented
         |@toolDefinition(name = "expected")
         |trait Expected
         |""".stripMargin
    val nonexistentRoot =
      """|package example.invalid
         |@toolMiddleware(name = "nonexistent-root")
         |final class NonexistentRoot
         |    extends _root_.missing.PresentedMiddleware.Adapter[
         |      _root_.missing.ExpectedUnderlying
         |    ]
         |""".stripMargin
    val nonexistentNamed =
      """|package example.invalid
         |import missing.api.{PresentedMiddleware, ExpectedUnderlying}
         |@toolMiddleware(name = "nonexistent-named")
         |final class NonexistentNamed extends PresentedMiddleware.Adapter[ExpectedUnderlying]
         |""".stripMargin
    val wrongWildcard =
      """|package example.invalid
         |import missing.api._
         |@toolMiddleware(name = "wrong-wildcard")
         |final class WrongWildcard extends PresentedMiddleware.Adapter[ExpectedUnderlying]
         |""".stripMargin
    val rootWithFallbackWildcard =
      """|package example.invalid
         |import example.api._
         |@toolMiddleware(name = "root-with-fallback")
         |final class RootWithFallback
         |    extends _root_.missing.PresentedMiddleware.Adapter[
         |      _root_.missing.ExpectedUnderlying
         |    ]
         |""".stripMargin
    val namedWithFallbackWildcard =
      """|package example.invalid
         |import missing.api.{PresentedMiddleware, ExpectedUnderlying}
         |import example.api._
         |@toolMiddleware(name = "named-with-fallback")
         |final class NamedWithFallback extends PresentedMiddleware.Adapter[ExpectedUnderlying]
         |""".stripMargin
    val qualifiedWithFallbackWildcard =
      """|package example.invalid
         |import example.api._
         |@toolMiddleware(name = "qualified-with-fallback")
         |final class QualifiedWithFallback
         |    extends missing.PresentedMiddleware.Adapter[missing.ExpectedUnderlying]
         |""".stripMargin

    List(
      "NonexistentRoot.scala"               -> nonexistentRoot,
      "NonexistentNamed.scala"              -> nonexistentNamed,
      "WrongWildcard.scala"                 -> wrongWildcard,
      "RootWithFallback.scala"              -> rootWithFallbackWildcard,
      "NamedWithFallback.scala"             -> namedWithFallbackWildcard,
      "QualifiedWithFallbackWildcard.scala" -> qualifiedWithFallbackWildcard
    ).foreach { case (path, source) =>
      val result = discover("Tools.scala" -> tools, path -> source)
      assert(result.errors.exists(_.message.contains("unresolved presented tool")), result.errors.mkString("; "))
      assert(result.errors.exists(_.message.contains("unresolved expected underlying")), result.errors.mkString("; "))
    }
  }

  test("rejects an unrelated type whose name ends in UniversalToolMiddleware") {
    val middleware =
      """|package example.invalid
         |@universalToolMiddleware(name = "wrong-universal")
         |final class WrongUniversal extends unrelated.UniversalToolMiddleware
         |""".stripMargin

    val result = discover("WrongUniversal.scala" -> middleware)
    assert(result.errors.exists(_.message.contains("golem.tool.UniversalToolMiddleware")))
    assertEquals(result.universalToolMiddlewares, Nil)
  }

  test("reports every phase-zero invalid middleware fixture with a targeted diagnostic") {
    val expectedMessage = Map(
      "constructor-argument"  -> "zero-argument primary constructor",
      "generic"               -> "must not declare type parameters",
      "wrong-parent"          -> "must directly extend",
      "missing-name"          -> "non-empty name",
      "unresolved-underlying" -> "unresolved expected underlying"
    )

    expectedMessage.foreach { case (fixtureName, expected) =>
      val result = discover(
        "Tools.scala"         -> ToolMiddlewareContractFixtures.toolDefinitions,
        s"$fixtureName.scala" -> ToolMiddlewareContractFixtures.invalidMiddlewareSources(fixtureName)
      )
      assert(
        result.errors.exists(_.message.contains(expected)),
        s"$fixtureName did not report `$expected`: ${result.errors.mkString("; ")}"
      )
    }
  }

  test("rejects non-class, abstract, secondary-constructor, and invalid universal definitions") {
    val invalid =
      """|package example.middleware
         |@toolMiddleware(name = "trait")
         |trait TraitMiddleware
         |@universalToolMiddleware(name = "object")
         |object ObjectMiddleware
         |@toolMiddleware(name = "abstract")
         |abstract class AbstractMiddleware extends PublicEchoMiddleware
         |@toolMiddleware(name = "secondary")
         |final class SecondaryMiddleware() extends PublicEchoMiddleware {
         |  def this(value: String) = this()
         |}
         |@universalToolMiddleware(name = "wrong-universal")
         |final class WrongUniversal extends PublicEchoMiddleware
         |""".stripMargin

    val result = discover(
      "Tools.scala"   -> ToolMiddlewareContractFixtures.toolDefinitions,
      "Invalid.scala" -> invalid
    )
    val messages = result.errors.map(_.message).mkString("\n")

    assert(messages.contains("not a trait"), messages)
    assert(messages.contains("not an object"), messages)
    assert(messages.contains("must be concrete, not abstract"), messages)
    assert(messages.contains("must not declare secondary constructors"), messages)
    assert(messages.contains("must directly extend `golem.tool.UniversalToolMiddleware`"), messages)
  }

  test("rejects duplicate names across monomorphic and universal middleware") {
    val duplicates =
      """|package example.middleware
         |@toolMiddleware(name = "duplicate")
         |final class MonomorphicDuplicate extends PublicEchoMiddleware
         |@universalToolMiddleware(name = "duplicate")
         |final class UniversalDuplicate extends golem.tool.UniversalToolMiddleware
         |""".stripMargin

    val result = discover(
      "Tools.scala"      -> ToolMiddlewareContractFixtures.toolDefinitions,
      "Duplicates.scala" -> duplicates
    )

    assert(result.errors.exists(_.message.contains("Duplicate tool middleware name `duplicate`")))
  }

  test("middleware source and surface hashes change with source metadata") {
    def discovered(description: String): SourceDiscovery.ToolMiddlewareImpl = {
      val middleware =
        s"""|package example.middleware
            |@toolMiddleware(name = "hashed")
            |@description("$description")
            |final class Hashed extends PublicEchoMiddleware
            |""".stripMargin
      discover(
        "Tools.scala"  -> ToolMiddlewareContractFixtures.toolDefinitions,
        "Hashed.scala" -> middleware
      ).toolMiddlewares.head
    }

    val first  = discovered("first")
    val second = discovered("second")
    assertNotEquals(first.sourceHash, second.sourceHash)
    assertNotEquals(first.surfaceHash, second.surfaceHash)
  }

  test("middleware source and surface hashes participate in generated fingerprints") {
    def generated(description: String): String = {
      val source =
        s"""|package example.fingerprint
            |@toolDefinition(name = "hash-tool")
            |trait HashTool
            |@toolImplementation()
            |final class HashToolImpl extends HashTool
            |@toolMiddleware(name = "hash-middleware")
            |@description("$description")
            |final class HashMiddleware extends HashToolMiddleware
            |""".stripMargin
      val result = CodegenPipeline.run(
        discover("Fingerprint.scala" -> source),
        Some("example"),
        rpcEnabled = false
      )
      result.autoRegister.get.files
        .find(_.relativePath.endsWith("__GolemAutoRegister_example_fingerprint.scala"))
        .get
        .content
    }

    assertNotEquals(generated("first"), generated("second"))
  }

  test("the shared pipeline fails fatally with discovery diagnostics") {
    val discovered = discover(
      "Tools.scala"   -> ToolMiddlewareContractFixtures.toolDefinitions,
      "Invalid.scala" -> ToolMiddlewareContractFixtures.invalidMiddlewareSources("constructor-argument")
    )

    val error = intercept[CodegenPipeline.PipelineException] {
      CodegenPipeline.run(discovered, Some("example"), rpcEnabled = true)
    }
    assert(error.getMessage.contains("Invalid.scala"), error.getMessage)
    assert(error.getMessage.contains("zero-argument primary constructor"), error.getMessage)
  }
}
