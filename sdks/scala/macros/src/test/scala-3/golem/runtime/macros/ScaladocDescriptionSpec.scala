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

package golem.runtime.macros

import golem.runtime.annotations.{agentDefinition, description}
import zio.test.*

import scala.concurrent.Future

/**
 * Verifies that Scaladoc comments are used as agent type / constructor / method
 * descriptions when no `@description` annotation is present, and that
 * annotations keep precedence over Scaladoc.
 */
object ScaladocDescriptionSpec extends ZIOSpecDefault {

  /**
   * A documented agent.
   *
   * It has a longer description spanning multiple lines.
   *
   * @note
   *   this tag must not leak into the description
   */
  @agentDefinition("scaladoc-agent")
  trait ScaladocAgent {

    /** Identifies a scaladoc agent instance. */
    class Id(val name: String)

    /** Echoes the input string. */
    def echo(message: String): Future[String]

    def undocumented(x: Int): Future[Int]
  }

  /** Scaladoc that must lose against the annotations. */
  @agentDefinition("annotated-agent")
  @description("Annotation wins on the trait.")
  trait AnnotatedAgent {
    class Id()

    /** Scaladoc that must lose on the method. */
    @description("Annotation wins on the method.")
    def act(): Future[Unit]
  }

  @agentDefinition("undocumented-agent")
  trait UndocumentedAgent {
    class Id()

    def act(): Future[Unit]
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("ScaladocDescriptionSpec")(
      suite("scaladoc-based descriptions")(
        test("trait scaladoc becomes the agent description, tags stripped") {
          val metadata = AgentMacros.agentMetadata[ScaladocAgent]
          assertTrue(
            metadata.description.contains(
              "A documented agent.\n\nIt has a longer description spanning multiple lines."
            )
          )
        },
        test("method scaladoc becomes the method description") {
          val metadata = AgentMacros.agentMetadata[ScaladocAgent]
          val echo     = metadata.methods.find(_.name == "echo").get
          assertTrue(echo.description.contains("Echoes the input string."))
        },
        test("undocumented method has no description") {
          val metadata = AgentMacros.agentMetadata[ScaladocAgent]
          val m        = metadata.methods.find(_.name == "undocumented").get
          assertTrue(m.description.isEmpty)
        },
        test("Id class scaladoc becomes the constructor description") {
          val metadata = AgentMacros.agentMetadata[ScaladocAgent]
          assertTrue(metadata.constructor.description == "Identifies a scaladoc agent instance.")
        }
      ),
      suite("annotation precedence")(
        test("@description on the trait wins over scaladoc") {
          val metadata = AgentMacros.agentMetadata[AnnotatedAgent]
          assertTrue(metadata.description.contains("Annotation wins on the trait."))
        },
        test("@description on a method wins over scaladoc") {
          val metadata = AgentMacros.agentMetadata[AnnotatedAgent]
          val act      = metadata.methods.find(_.name == "act").get
          assertTrue(act.description.contains("Annotation wins on the method."))
        },
        test("undocumented Id class falls back to the trait description") {
          val metadata = AgentMacros.agentMetadata[AnnotatedAgent]
          assertTrue(metadata.constructor.description == "Annotation wins on the trait.")
        }
      ),
      suite("no docs at all")(
        test("behaves as before: no description, constructor falls back to type name") {
          val metadata = AgentMacros.agentMetadata[UndocumentedAgent]
          assertTrue(
            metadata.description.isEmpty,
            metadata.constructor.description == "undocumented-agent"
          )
        }
      ),
      suite("Scaladoc parser")(
        test("single-line comment") {
          assertTrue(Scaladoc.clean("/** Does the thing. */").contains("Does the thing."))
        },
        test("multi-line comment with margin stars") {
          val raw = "/** First line.\n *\n * Second paragraph\n * continues here.\n */"
          assertTrue(Scaladoc.clean(raw).contains("First line.\n\nSecond paragraph\ncontinues here."))
        },
        test("tag sections are dropped") {
          val raw = "/** Prose.\n * @param x the x\n * @return something\n */"
          assertTrue(Scaladoc.clean(raw).contains("Prose."))
        },
        test("tags-only comment yields None") {
          val raw = "/** @param x the x */"
          assertTrue(Scaladoc.clean(raw).isEmpty)
        },
        test("email-like text is not treated as a tag") {
          val raw = "/** Contact @ home. */"
          assertTrue(Scaladoc.clean(raw).contains("Contact @ home."))
        },
        test("summary and description split") {
          val (summary, desc) = Scaladoc.summaryAndDescription("First line\njoined.\n\nRest of\nthe text.")
          assertTrue(
            summary == "First line joined.",
            desc == "Rest of\nthe text."
          )
        }
      )
    )
}
