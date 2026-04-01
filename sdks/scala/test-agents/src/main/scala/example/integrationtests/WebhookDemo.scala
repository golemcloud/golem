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

import golem.runtime.annotations.{agentDefinition, description, endpoint}
import golem.BaseAgent
import zio.blocks.schema.Schema

import scala.concurrent.Future

final case class WebhookEvent(message: String, count: Int)
object WebhookEvent {
  implicit val schema: Schema[WebhookEvent] = Schema.derived
}

@agentDefinition(
  mount = "/api/webhook-demo/{value}",
  webhookSuffix = "/incoming",
  cors = Array("*")
)
@description("Demonstrates webhook creation and awaiting webhook payloads")
trait WebhookDemo extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/create")
  @description("Creates a webhook and returns the webhook URL")
  def createWebhookUrl(): Future[String]

  @endpoint(method = "GET", path = "/await")
  @description("Awaits the webhook payload and returns the decoded JSON")
  def awaitWebhookJson(): Future[String]
}
