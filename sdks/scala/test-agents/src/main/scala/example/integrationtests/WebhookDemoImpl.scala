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

import golem.HostApi
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@agentImplementation()
final class WebhookDemoImpl(@unused private val key: String) extends WebhookDemo {

  private var pending: Option[HostApi.WebhookHandler] = None

  override def createWebhookUrl(): Future[String] = Future.successful {
    val webhook = HostApi.createWebhook()
    pending = Some(webhook)
    webhook.url
  }

  override def awaitWebhookJson(): Future[String] =
    pending match {
      case Some(handler) =>
        pending = None
        handler.await().map { payload =>
          val event = payload.json[WebhookEvent]()
          s"message=${event.message},count=${event.count}"
        }
      case None =>
        Future.successful("no pending webhook")
    }
}
