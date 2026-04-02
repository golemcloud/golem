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

import golem.runtime.annotations.{agentDefinition, id, description, endpoint, header}
import golem.BaseAgent

import scala.concurrent.Future

// ---------------------------------------------------------------------------
// Single-element constructor: class Id(val value: String)
//
// The mount path variable must be named {value} — matching the Id
// parameter name.
// ---------------------------------------------------------------------------
@agentDefinition(mount = "/api/weather/{value}", cors = Array("*"))
@description("A weather agent demonstrating code-first HTTP routes with a single constructor parameter")
trait WeatherAgent extends BaseAgent {

  class Id(val value: String)

  @endpoint(method = "GET", path = "/current/{city}")
  @description("Returns current weather for a city")
  def getWeather(city: String): Future[String]

  @endpoint(method = "GET", path = "/search?q={query}&limit={n}")
  @description("Search weather data")
  def search(query: String, n: Int): Future[String]

  @endpoint(method = "POST", path = "/report")
  @description("Submit a weather report with tenant header")
  def submitReport(@header("X-Tenant") tenantId: String, data: String): Future[String]

  @endpoint(method = "GET", path = "/greet/{name}/{*filePath}")
  @description("Catch-all path example")
  def greetWithPath(name: String, filePath: String): Future[String]

  @endpoint(method = "GET", path = "/")
  @description("Root endpoint")
  def root(): Future[String]

  @endpoint(method = "GET", path = "/public", auth = false)
  @description("Public endpoint with auth disabled")
  def publicEndpoint(): Future[String]
}

// ---------------------------------------------------------------------------
// Tuple constructor: class Id(val arg0: String, val arg1: Int)
//
// Mount path variables must be named {arg0}, {arg1}, etc., matching the
// Id parameter names.
// ---------------------------------------------------------------------------
@agentDefinition(mount = "/api/inventory/{arg0}/{arg1}")
@description("An inventory agent demonstrating tuple constructor parameters")
trait InventoryAgent extends BaseAgent {

  class Id(val arg0: String, val arg1: Int)

  @endpoint(method = "GET", path = "/stock")
  @description("Get stock level for this warehouse/zone")
  def getStock(): Future[String]

  @endpoint(method = "GET", path = "/item/{itemId}")
  @description("Get a specific item")
  def getItem(itemId: String): Future[String]
}

// ---------------------------------------------------------------------------
// Named constructor using @id annotation:
//
// Instead of the default `class Id(...)`, you can use any class name
// with the @id annotation. Mount path variables match the
// annotated class's parameter names: {region}, {catalog}.
// ---------------------------------------------------------------------------
@agentDefinition(mount = "/api/catalog/{region}/{catalog}")
@description("A catalog agent demonstrating @id with a custom class name")
trait CatalogAgent extends BaseAgent {

  @id
  class CatalogParams(val region: String, val catalog: String)

  @endpoint(method = "GET", path = "/search?q={query}")
  @description("Search the catalog")
  def search(query: String): Future[String]

  @endpoint(method = "GET", path = "/item/{itemId}")
  @description("Get a specific item")
  def getItem(itemId: String): Future[String]
}

// ---------------------------------------------------------------------------
// Phantom agent with webhook suffix
// ---------------------------------------------------------------------------
@agentDefinition(
  mount = "/webhook/{agent-type}/{value}",
  phantomAgent = true,
  webhookSuffix = "/{agent-type}/events"
)
@description("Demonstrates phantom agent with webhook suffix")
trait WebhookAgent extends BaseAgent {
  class Id(val value: String)
  @endpoint(method = "POST", path = "/receive")
  def receive(payload: String): Future[String]
}
