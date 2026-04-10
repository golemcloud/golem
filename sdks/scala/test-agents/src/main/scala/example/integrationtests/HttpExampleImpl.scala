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

import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class WeatherAgentImpl(@unused private val apiKey: String) extends WeatherAgent {

  override def getWeather(city: String): Future[String] =
    Future.successful(s"Sunny in $city")

  override def search(query: String, n: Int): Future[String] =
    Future.successful(s"Found $n results for '$query'")

  override def submitReport(tenantId: String, data: String): Future[String] =
    Future.successful(s"Report from tenant $tenantId: $data")

  override def greetWithPath(name: String, filePath: String): Future[String] =
    Future.successful(s"Hello $name, path: $filePath")

  override def root(): Future[String] =
    Future.successful("Welcome to the Weather API")

  override def publicEndpoint(): Future[String] =
    Future.successful("Public endpoint")
}

@agentImplementation()
final class InventoryAgentImpl(
  @unused private val warehouse: String,
  @unused private val zone: Int
) extends InventoryAgent {

  override def getStock(): Future[String] =
    Future.successful(s"Stock for warehouse=$warehouse, zone=$zone: 42 items")

  override def getItem(itemId: String): Future[String] =
    Future.successful(s"Item $itemId in warehouse=$warehouse, zone=$zone")
}

@agentImplementation()
final class CatalogAgentImpl(
  @unused private val region: String,
  @unused private val catalog: String
) extends CatalogAgent {

  override def search(query: String): Future[String] =
    Future.successful(s"Searching $catalog in $region for '$query'")

  override def getItem(itemId: String): Future[String] =
    Future.successful(s"Item $itemId from $catalog in $region")
}

@agentImplementation()
final class WebhookAgentImpl(@unused private val key: String) extends WebhookAgent {
  override def receive(payload: String): Future[String] =
    Future.successful(s"Received: $payload")
}
