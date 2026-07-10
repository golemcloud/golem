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

package golem

import golem.runtime.autowire.{AgentImplementation, MethodBinding}
import golem.runtime.ParameterMetadata
import golem.schema.{SchemaGraph, SchemaTypeBody}
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future

/**
 * Schema-verification cases that need `Schema.derived` for stdlib `Either`,
 * which the zio-blocks macro only supports on Scala 3 (the Scala 2.13 macro
 * cannot derive these stdlib generics). The cross-version cases live in
 * [[SchemaVerificationSpec]].
 */
object SchemaVerificationScala3Spec extends ZIOSpecDefault {

  implicit val eitherStringIntSchema: Schema[Either[String, Int]] = Schema.derived

  @agentDefinition("schema-verify-either-agent")
  trait SchemaVerifyEitherAgent extends BaseAgent {
    class Id()
    def eitherMethod(e: Either[String, Int]): Future[Either[String, Int]]
  }

  @agentImplementation()
  final class SchemaVerifyEitherAgentImpl() extends SchemaVerifyEitherAgent {
    override def eitherMethod(e: Either[String, Int]): Future[Either[String, Int]] = Future.successful(e)
  }

  private lazy val defn = AgentImplementation.registerClass[SchemaVerifyEitherAgent, SchemaVerifyEitherAgentImpl]

  private def findMethod(name: String): MethodBinding[SchemaVerifyEitherAgent] =
    defn.methodMetadata.find(_.metadata.name == name).getOrElse(sys.error(s"method not found: $name"))

  private def params(methodName: String): List[ParameterMetadata] =
    findMethod(methodName).metadata.input.userSupplied

  /**
   * The effective root body of a graph, dereferencing a top-level named ref.
   */
  private def rootBody(g: SchemaGraph): SchemaTypeBody = g.root.body match {
    case SchemaTypeBody.RefType(id) => g.defs(id).body.body
    case other                      => other
  }

  /** The schema body of the first (single) parameter of a method. */
  private def firstParamBody(methodName: String): SchemaTypeBody =
    rootBody(params(methodName).head.graph)

  def spec = suite("SchemaVerificationScala3Spec")(
    test("either method produces result schema body") {
      assertTrue(firstParamBody("eitherMethod") match {
        case _: SchemaTypeBody.ResultType => true
        case _                            => false
      })
    }
  )
}
