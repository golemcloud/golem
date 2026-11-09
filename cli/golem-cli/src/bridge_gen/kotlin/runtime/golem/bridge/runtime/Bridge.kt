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
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.util.concurrent.CancellationException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionException

/**
 * The REST transport the generated clients of this project share. It calls the worker service's `create-agent` and `invoke-agent`
 * endpoints with `java.net.http.HttpClient` and the runtime's own JSON, so it needs nothing beyond the JDK. Every call has a blocking form
 * and an `Async` form that returns a [CompletableFuture].
 */
object Bridge {
    private val httpClient: HttpClient by lazy { HttpClient.newHttpClient() }

    /** Resolves (creating it if necessary) a durable agent, and returns its identity. */
    fun createAgent(configuration: Configuration, agentTypeName: String, parameters: SchemaValue, phantomId: String?): AgentId =
        await(createAgentAsync(configuration, agentTypeName, parameters, phantomId))

    fun createAgentAsync(configuration: Configuration, agentTypeName: String, parameters: SchemaValue, phantomId: String?): CompletableFuture<AgentId> {
        // Rendering can fail on an unencodable value. An `Async` call reports that
        // through the future, like every other failure, rather than throwing here.
        val body = try {
            BridgeProtocol.renderCreateAgentRequest(configuration, agentTypeName, parameters, phantomId)
        } catch (e: BridgeException) {
            return CompletableFuture.failedFuture(e)
        }
        return send(configuration, "create-agent", body).thenApply { complete("create-agent", it, BridgeProtocol::decodeCreateAgentResponse) }
    }

    /** Invokes a method of a resolved agent; [mode] is `await` or `schedule`. */
    fun invokeAgent(resolved: ResolvedAgent, methodName: String, methodParameters: SchemaValue, mode: String, scheduleAt: String?): AgentInvocationResult =
        await(invokeAgentAsync(resolved, methodName, methodParameters, mode, scheduleAt))

    fun invokeAgentAsync(
        resolved: ResolvedAgent,
        methodName: String,
        methodParameters: SchemaValue,
        mode: String,
        scheduleAt: String?,
    ): CompletableFuture<AgentInvocationResult> {
        val body = try {
            BridgeProtocol.renderInvocationRequest(resolved, methodName, methodParameters, mode, scheduleAt)
        } catch (e: BridgeException) {
            return CompletableFuture.failedFuture(e)
        }
        return send(resolved.configuration, "invoke-agent", body).thenApply { complete("invoke-agent", it, BridgeProtocol::decodeInvocationResult) }
    }

    /**
     * Invokes a stream-bearing method of a resolved agent and blocks for its result. The streams of the call travel over one WebSocket
     * session, so [methodParameters] renders inside that session and must be passed unevaluated.
     */
    fun invokeStreamingAgent(resolved: ResolvedAgent, methodName: String, methodParameters: () -> SchemaValue): AgentInvocationResult =
        awaitStreaming(invokeStreamingAgentAsync(resolved, methodName, methodParameters))

    fun invokeStreamingAgentAsync(
        resolved: ResolvedAgent,
        methodName: String,
        methodParameters: () -> SchemaValue,
    ): CompletableFuture<AgentInvocationResult> = try {
        StreamSession.invoke(resolved, methodName, methodParameters)
    } catch (e: BridgeException) {
        CompletableFuture.failedFuture(e)
    }

    private fun send(configuration: Configuration, endpoint: String, body: String): CompletableFuture<HttpResponse<String>> {
        val server = configuration.server
        val uri = try {
            URI.create("${server.url.trimEnd('/')}/v1/agents/$endpoint")
        } catch (e: IllegalArgumentException) {
            return CompletableFuture.failedFuture(BridgeException("Invalid server URL '${server.url}'", e))
        }
        // `header` rejects a value holding a control character or a non-ASCII byte,
        // which is a bad token rather than a bad URL.
        val request = try {
            HttpRequest.newBuilder()
                .uri(uri)
                .header("Content-Type", "application/json")
                .header("Authorization", "Bearer ${server.token}")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build()
        } catch (e: IllegalArgumentException) {
            return CompletableFuture.failedFuture(
                BridgeException("The $endpoint request could not be built; check the server token for control or non-ASCII characters", e)
            )
        }
        return httpClient.sendAsync(request, HttpResponse.BodyHandlers.ofString())
    }

    private fun <A> complete(endpoint: String, response: HttpResponse<String>, decode: (Json) -> A): A {
        val status = response.statusCode()
        if (status !in 200..299) throw BridgeException("Golem $endpoint request failed with HTTP $status: ${response.body()}")
        return try {
            decode(Json.parse(response.body()))
        } catch (e: BridgeException) {
            throw BridgeException("Failed to decode $endpoint response: ${e.message}", e)
        }
    }

    private fun <A> await(future: CompletableFuture<A>): A = try {
        future.join()
    } catch (e: CompletionException) {
        throw asBridgeFailure(e)
    } catch (e: CancellationException) {
        throw BridgeException("Golem request was cancelled", e)
    }

    private fun asBridgeFailure(e: CompletionException): BridgeException =
        e.cause as? BridgeException ?: BridgeException("Golem request failed: ${e.cause?.message ?: e.message}", e.cause ?: e)

    /** Like [await], but a stream terminal the invocation failed with reaches the caller as it is, so its protocol code survives. */
    private fun <A> awaitStreaming(future: CompletableFuture<A>): A = try {
        future.join()
    } catch (e: CompletionException) {
        throw e.cause as? AgentStreamTerminal ?: asBridgeFailure(e)
    } catch (e: CancellationException) {
        throw BridgeException("Golem request was cancelled", e)
    }
}
