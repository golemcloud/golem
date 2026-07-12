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

package golem.codegen

import java.nio.charset.StandardCharsets
import java.nio.file.{Files, Path, Paths}

class PublicationMatrixSpec extends munit.FunSuite {

  private val scala212Switch = "++2.12.21"

  private def repositoryRoot: Path =
    Iterator
      .iterate(Paths.get("").toAbsolutePath)(_.getParent)
      .takeWhile(_ != null)
      .find(path => Files.isDirectory(path.resolve(".github")) && Files.isDirectory(path.resolve("sdks/scala")))
      .getOrElse(fail("Could not locate the repository root"))

  private def read(path: String): String =
    new String(Files.readAllBytes(repositoryRoot.resolve(path)), StandardCharsets.UTF_8)

  private def assertPublishesBoth(command: String, task: String, source: String): Unit = {
    val switchIndex = command.indexOf(scala212Switch)
    assert(switchIndex >= 0, s"$source does not switch to the sbt host's Scala 2.12 version")

    val scala3Tasks   = command.substring(0, switchIndex)
    val scala212Tasks = command.substring(switchIndex)
    assert(scala3Tasks.contains(task), s"$source does not publish the Scala 3 codegen artifact")
    assert(scala212Tasks.contains(task), s"$source does not publish the Scala 2.12 codegen artifact")
  }

  test("release publishes codegen for Scala 3 and the Scala 2.12 sbt host") {
    val workflow = read(".github/workflows/publish-golem-scala.yaml")
    val command  = workflow.linesIterator
      .find(_.contains("CI_RELEASE:"))
      .getOrElse(fail("CI_RELEASE was not found in the Scala publication workflow"))

    assertPublishesBoth(command, "codegen/publishSigned", ".github/workflows/publish-golem-scala.yaml")
  }

  test("local CI publication publishes codegen for Scala 3 and the Scala 2.12 sbt host") {
    val workflowPaths = Seq(
      ".github/workflows/ci.yaml",
      ".github/actions/run-skill-harness/action.yml"
    )

    workflowPaths.foreach { path =>
      val commands =
        read(path).linesIterator.filter(line => line.contains("sbt -batch") && line.contains("publishLocal")).toList
      assert(commands.nonEmpty, s"No Scala local-publication command found in $path")
      commands.foreach(command => assertPublishesBoth(command, "codegen/publishLocal", path))
    }
  }
}
