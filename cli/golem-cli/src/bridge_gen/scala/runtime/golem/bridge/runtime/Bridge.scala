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

package golem.bridge.runtime

import golem.bridge.runtime.json.Json

import java.net.URI
import java.net.http.{HttpClient, HttpRequest, HttpResponse}
import java.util.concurrent.CompletableFuture
import scala.concurrent.{Future, Promise}

/**
 * The REST transport shared by all generated bridge clients in this project.
 *
 * Calls the worker service's `create-agent` and `invoke-agent` endpoints over
 * `java.net.http.HttpClient`, with hand-rolled JSON bodies — no third-party
 * dependencies. All methods are non-blocking and return `Future`s completed on
 * the [[Configuration]]'s `ExecutionContext`.
 */
object Bridge {

  private lazy val httpClient: HttpClient = HttpClient.newHttpClient()

  /** Creates (or gets) an agent and returns its identity. */
  def createAgent(
    configuration: Configuration,
    agentTypeName: String,
    parameters: SchemaValue,
    phantomId: Option[String],
    config: List[AgentConfigEntry]
  ): Future[CreateAgentResponse] = {
    implicit val ec: scala.concurrent.ExecutionContext = configuration.executionContext
    val request = CreateAgentRequest(
      appName = configuration.appName,
      envName = configuration.envName,
      agentTypeName = agentTypeName,
      parameters = parameters,
      phantomId = phantomId,
      config = config
    )
    val body = BridgeProtocol.encodeCreateAgentRequest(request).render
    send(configuration, "create-agent", body, None).flatMap { response =>
      complete("create-agent", response, BridgeProtocol.decodeCreateAgentResponse)
    }
  }

  /** Invokes a method on a resolved agent. */
  def invokeAgent(
    resolved: ResolvedAgent,
    methodName: String,
    methodParameters: SchemaValue,
    mode: String,
    scheduleAt: Option[String]
  ): Future[AgentInvocationResult] = {
    val configuration = resolved.configuration
    implicit val ec: scala.concurrent.ExecutionContext = configuration.executionContext
    val request = AgentInvocationRequest(
      appName = configuration.appName,
      envName = configuration.envName,
      agentTypeName = resolved.agentTypeName,
      parameters = resolved.parameters,
      phantomId = resolved.phantomId,
      methodName = methodName,
      methodParameters = methodParameters,
      mode = mode,
      scheduleAt = scheduleAt,
      idempotencyKey = None
    )
    val body = BridgeProtocol.encodeAgentInvocationRequest(request).render
    send(configuration, "invoke-agent", body, request.idempotencyKey).flatMap { response =>
      complete("invoke-agent", response, BridgeProtocol.decodeAgentInvocationResult)
    }
  }

  private def send(
    configuration: Configuration,
    endpoint: String,
    body: String,
    idempotencyKey: Option[String]
  ): Future[HttpResponse[String]] = {
    val server  = configuration.server
    val baseUrl = server.url.stripSuffix("/")
    val builder = HttpRequest
      .newBuilder()
      .uri(URI.create(s"$baseUrl/v1/agents/$endpoint"))
      .header("Content-Type", "application/json")
      .header("Authorization", s"Bearer ${server.token}")
    idempotencyKey.foreach(key => builder.header("Idempotency-Key", key))
    val httpRequest = builder
      .POST(HttpRequest.BodyPublishers.ofString(body))
      .build()
    toScala(httpClient.sendAsync(httpRequest, HttpResponse.BodyHandlers.ofString()))
  }

  private def complete[A](
    endpoint: String,
    response: HttpResponse[String],
    decode: Json => Either[String, A]
  )(implicit ec: scala.concurrent.ExecutionContext): Future[A] = {
    val status = response.statusCode()
    if (status >= 200 && status < 300) {
      val decoded = Json.parse(response.body()).flatMap(decode)
      decoded match {
        case Right(value) => Future.successful(value)
        case Left(error) =>
          Future.failed(BridgeException(s"Failed to decode $endpoint response: $error"))
      }
    } else {
      Future.failed(
        BridgeException(s"Golem $endpoint request failed with HTTP $status: ${response.body()}")
      )
    }
  }

  private def toScala[A](future: CompletableFuture[A]): Future[A] = {
    val promise = Promise[A]()
    future.whenComplete { (value: A, error: Throwable) =>
      if (error != null) promise.failure(error)
      else promise.success(value)
      ()
    }
    promise.future
  }
}
