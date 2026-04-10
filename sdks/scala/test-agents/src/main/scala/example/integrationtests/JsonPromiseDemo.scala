/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package example.integrationtests

import golem.runtime.annotations.{agentDefinition, description}
import golem.BaseAgent
import zio.blocks.schema.Schema

import scala.concurrent.Future

final case class PromisePayload(message: String, count: Int)
object PromisePayload {
  implicit val schema: Schema[PromisePayload] = Schema.derived
}

@agentDefinition()
@description("Demonstrates JSON-typed promises and blocking promise await.")
trait JsonPromiseDemo extends BaseAgent {

  class Id(val value: String)

  @description("Creates a promise, completes with JSON, and awaits the JSON result.")
  def jsonRoundtrip(): Future[String]

  @description("Creates a promise, completes with raw bytes, and uses blocking await.")
  def blockingDemo(): Future[String]
}
