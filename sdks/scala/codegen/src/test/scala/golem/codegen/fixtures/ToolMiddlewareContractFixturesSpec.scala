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

package golem.codegen.fixtures

import golem.codegen.discovery.SourceDiscovery
import golem.codegen.rpc.ToolRpcCodegen

import scala.meta._
import scala.meta.parsers._

import java.nio.charset.StandardCharsets
import java.security.MessageDigest

class ToolMiddlewareContractFixturesSpec extends munit.FunSuite {

  private def parses(source: String): Boolean =
    dialects.Scala3(source).parse[Source].toOption.nonEmpty

  private def sha256(value: String): String =
    MessageDigest
      .getInstance("SHA-256")
      .digest(value.getBytes(StandardCharsets.UTF_8))
      .map(byte => f"${byte & 0xff}%02x")
      .mkString

  test("middleware contract sources are valid Scala 3 syntax") {
    val sources = List(
      ToolMiddlewareContractFixtures.toolDefinitions,
      ToolMiddlewareContractFixtures.transparentMiddleware,
      ToolMiddlewareContractFixtures.adapterMiddleware,
      ToolMiddlewareContractFixtures.universalMiddleware
    )

    sources.foreach(source => assert(parses(source), source))
  }

  test("negative middleware fixtures isolate semantic rather than parse failures") {
    ToolMiddlewareContractFixtures.invalidMiddlewareSources.foreach { case (name, source) =>
      assert(parses(source), s"$name fixture did not parse:\n$source")
    }
  }

  test("ordinary generated client public surface is frozen before middleware projection refactoring") {
    val source     = ToolMiddlewareContractFixtures.ordinaryClientBaselineSource
    val discovered = SourceDiscovery.discover(Seq(SourceDiscovery.SourceInput("BaselineTool.scala", source)))
    val result     = ToolRpcCodegen.generate(discovered.tools.toList, discovered.objects)
    val content    = result.files
      .find(_.relativePath == "example/baseline/BaselineToolClient.scala")
      .getOrElse(fail(result.toString))
      .content

    ToolMiddlewareContractFixtures.ordinaryClientSurface.foreach { expected =>
      assert(content.contains(expected), s"Missing frozen client surface:\n$expected\n\n$content")
    }
    assertEquals(
      sha256(content),
      ToolMiddlewareContractFixtures.ordinaryClientSnapshotSha256,
      s"Complete ordinary client snapshot changed:\n$content"
    )
  }
}
