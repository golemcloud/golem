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

import java.net.{URI, URLEncoder}
import java.net.http.{HttpClient, HttpRequest, HttpResponse}
import java.nio.charset.StandardCharsets
import java.util.concurrent.{CompletableFuture, ConcurrentHashMap}
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.util.Try

/**
 * The REST transport shared by all generated bridge clients in this project.
 *
 * Calls the worker service's runtime API under
 * `/v1/runtime/envs/{environment}/agent-types/{type}` over
 * `java.net.http.HttpClient`, with hand-rolled JSON bodies — no third-party
 * dependencies. All methods are non-blocking and return `Future`s completed on
 * the [[Configuration]]'s `ExecutionContext`.
 *
 * The runtime API names the environment by ID, and a [[Configuration]] names
 * it by the application and environment names. [[environmentId]] resolves the
 * ID with `GET /v1/me/token` and `GET /v1/me/visible-environments`, keeps the
 * match in the account of the token, and caches it.
 */
object Bridge {

  private lazy val httpClient: HttpClient = HttpClient.newHttpClient()

  /**
   * The resolved environment IDs, keyed by the server URL without a trailing
   * slash, the token, and the application and environment names.
   */
  private[runtime] val environmentIds = new ConcurrentHashMap[(String, String, String, String), String]()

  /**
   * The ID of the configured environment of the configured application in the
   * account of the server token. A successful resolution is cached for the
   * server, token and names. `timeoutMillis` bounds each lookup request.
   */
  def environmentId(configuration: Configuration, timeoutMillis: Option[Long] = None): Future[String] = {
    implicit val ec: ExecutionContext = configuration.executionContext
    val server = configuration.server
    val key    = (server.url.stripSuffix("/"), server.token, configuration.appName, configuration.envName)
    Option(environmentIds.get(key)) match {
      case Some(id) => Future.successful(id)
      case None     =>
        val query =
          s"app_name=${URLEncoder.encode(configuration.appName, StandardCharsets.UTF_8)}&env_name=${URLEncoder.encode(configuration.envName, StandardCharsets.UTF_8)}"
        val token        = send(configuration, "GET", "resolve-environment", "/v1/me/token", None, None, timeoutMillis)
        val environments =
          send(configuration, "GET", "resolve-environment", s"/v1/me/visible-environments?$query", None, None, timeoutMillis)
        for {
          tokenResponse        <- token
          environmentsResponse <- environments
          accountId            <- complete("resolve-environment", tokenResponse, BridgeProtocol.decodeTokenAccountId)
          found                <- complete(
                                    "resolve-environment",
                                    environmentsResponse,
                                    BridgeProtocol.decodeVisibleEnvironmentId(_, accountId, configuration.appName, configuration.envName)
                                  )
          id                   <- found match {
                                    case Some(id) => Future.successful(id)
                                    case None     =>
                                      Future.failed(
                                        BridgeException(
                                          s"Environment '${configuration.envName}' of application '${configuration.appName}' does not exist in the account of the token"
                                        )
                                      )
                                  }
        } yield {
          environmentIds.put(key, id)
          id
        }
    }
  }

  private def agentTypePath(configuration: Configuration, agentTypeName: String, suffix: String): Future[String] =
    environmentId(configuration).map { env =>
      s"/v1/runtime/envs/$env/agent-types/${URLEncoder.encode(agentTypeName, StandardCharsets.UTF_8).replace("+", "%20")}/$suffix"
    }(configuration.executionContext)

  /**
   * Creates an agent and completes after its initialization. A keyed create of
   * a current key fails with a [[BridgeConflictException]].
   */
  def createAgent(
    configuration: Configuration,
    agentTypeName: String,
    parameters: SchemaValue,
    keyed: Boolean,
    config: List[AgentConfigEntry],
    env: Map[String, String]
  ): Future[AgentReference] = {
    implicit val ec: ExecutionContext = configuration.executionContext
    val request                       = CreateAgentRequest(parameters, config, env, keyed)
    for {
      body     <- Future.fromTry(Try(BridgeProtocol.renderCreateAgentRequest(request)))
      path     <- agentTypePath(configuration, agentTypeName, "agents")
      response <- send(configuration, "POST", "create-agent", path, Some(body), None)
      created  <- complete("create-agent", response, BridgeProtocol.decodeAgentReference)
    } yield created
  }

  /** Returns the keyed agent of the parameters, and creates it when it is missing. */
  def upsertAgent(
    configuration: Configuration,
    agentTypeName: String,
    parameters: SchemaValue
  ): Future[AgentReference] = {
    implicit val ec: ExecutionContext = configuration.executionContext
    for {
      body     <- Future.fromTry(Try(BridgeProtocol.renderAgentKeyRequest(parameters)))
      path     <- agentTypePath(configuration, agentTypeName, "agents/upsert")
      response <- send(configuration, "POST", "upsert-agent", path, Some(body), None)
      upserted <- complete("upsert-agent", response, BridgeProtocol.decodeAgentReference)
    } yield upserted
  }

  /**
   * The existing agent of the type with the ID. It never creates the agent: an
   * unknown ID, or the ID of an agent of another type, fails with a
   * [[BridgeNotFoundException]].
   */
  def getAgent(configuration: Configuration, agentTypeName: String, agentId: AgentId): Future[AgentReference] = {
    implicit val ec: ExecutionContext = configuration.executionContext
    for {
      env      <- environmentId(configuration)
      response <- send(configuration, "GET", "get-agent", s"/v1/runtime/envs/$env/agents/${agentId.uuid}", None, None)
      metadata <- complete("get-agent", response, BridgeProtocol.decodeAgentMetadata)
      found    <- metadata match {
                    case (Some(`agentTypeName`), fingerprint) => Future.successful(AgentReference(agentId, fingerprint))
                    case _                                    =>
                      Future.failed(new BridgeNotFoundException("get-agent", s"agent $agentId is not a $agentTypeName agent"))
                  }
    } yield found
  }

  /**
   * The ID of the resolved agent. A remote of `upsert` asks the server the
   * first time, and the server creates the agent when it is missing.
   */
  def agentId(resolved: ResolvedAgent): Future[AgentId] =
    resolved.knownAgentId.get() match {
      case Some(id) => Future.successful(id)
      case None     =>
        implicit val ec: ExecutionContext = resolved.configuration.executionContext
        upsertAgent(resolved.configuration, resolved.agentTypeName, resolved.keyedParameters).map { reference =>
          resolved.knownAgentId.set(Some(reference.agentId))
          reference.agentId
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
    val configuration                 = resolved.configuration
    implicit val ec: ExecutionContext = configuration.executionContext
    for {
      request  <- Future.fromTry(Try(AgentInvocationRequest(
                    agentId = resolved.target.map(_.agentId),
                    parameters = if (resolved.target.isEmpty) Some(resolved.keyedParameters) else None,
                    method = methodName,
                    methodParameters = methodParameters,
                    mode = mode,
                    scheduleAt = scheduleAt,
                    expectedFingerprint = resolved.target.map(_.agentFingerprint)
                  )))
      body     <- Future.fromTry(Try(BridgeProtocol.renderAgentInvocationRequest(request)))
      path     <- agentTypePath(configuration, resolved.agentTypeName, "invoke")
      response <- send(configuration, "POST", "invoke-agent", path, Some(body), None)
      result   <- complete("invoke-agent", response, BridgeProtocol.decodeAgentInvocationResult)
    } yield result
  }

  private def send(
    configuration: Configuration,
    method: String,
    endpoint: String,
    path: String,
    body: Option[String],
    idempotencyKey: Option[String],
    timeoutMillis: Option[Long] = None
  ): Future[HttpResponse[String]] =
    Future.fromTry(Try {
      val server  = configuration.server
      val baseUrl = server.url.stripSuffix("/")
      val builder = HttpRequest
        .newBuilder()
        .uri(URI.create(s"$baseUrl$path"))
        .header("Authorization", s"Bearer ${server.token}")
      idempotencyKey.foreach(key => builder.header("Idempotency-Key", key))
      timeoutMillis.foreach(millis => builder.timeout(java.time.Duration.ofMillis(math.max(1L, millis))))
      body match {
        case Some(content) =>
          builder.header("Content-Type", "application/json").method(method, HttpRequest.BodyPublishers.ofString(content))
        case None => builder.method(method, HttpRequest.BodyPublishers.noBody())
      }
      builder.build()
    }).flatMap(request => toScala(httpClient.sendAsync(request, HttpResponse.BodyHandlers.ofString())))(
      configuration.executionContext
    )

  private def complete[A](
    endpoint: String,
    response: HttpResponse[String],
    decode: Json => Either[String, A]
  ): Future[A] = {
    val status = response.statusCode()
    if (status >= 200 && status < 300) {
      val decoded = Json.parse(response.body()).flatMap(decode)
      decoded match {
        case Right(value) => Future.successful(value)
        case Left(error)  =>
          Future.failed(BridgeException(s"Failed to decode $endpoint response: $error"))
      }
    } else
      Future.failed(status match {
        case 404 => new BridgeNotFoundException(endpoint, response.body())
        case 409 => new BridgeConflictException(endpoint, response.body())
        case _   => new BridgeHttpException(endpoint, status, response.body())
      })
  }

  private[runtime] def toScala[A](future: CompletableFuture[A]): Future[A] = {
    val promise = Promise[A]()
    future.whenComplete { (value: A, error: Throwable) =>
      if (error != null) promise.failure(error)
      else promise.success(value)
      ()
    }
    promise.future
  }

  /** Direct awaited invocation for a stream-bearing method. */
  def invokeStreamingAgent(
    resolved: ResolvedAgent,
    methodName: String,
    methodParameters: () => SchemaValue,
    constructorCodec: PublicValueCodec.Codec,
    inputCodec: PublicValueCodec.Codec,
    outputCodec: Option[PublicValueCodec.Codec]
  ): Future[AgentInvocationResult] =
    invokeStreamingAgentCancelable(
      resolved,
      methodName,
      methodParameters,
      constructorCodec,
      inputCodec,
      outputCodec
    )._1

  /** Like [[invokeStreamingAgent]], with a token that cancels the invocation. */
  def invokeStreamingAgentCancelable(
    resolved: ResolvedAgent,
    methodName: String,
    methodParameters: () => SchemaValue,
    constructorCodec: PublicValueCodec.Codec,
    inputCodec: PublicValueCodec.Codec,
    outputCodec: Option[PublicValueCodec.Codec]
  ): (Future[AgentInvocationResult], CancellationToken) =
    Try(
      StreamSession.invoke(
        resolved,
        methodName,
        methodParameters,
        constructorCodec,
        inputCodec,
        outputCodec
      )
    ).fold(error => (Future.failed(error), new CancellationToken(() => ())), identity)
}
