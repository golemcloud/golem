---
name: golem-make-http-request-kotlin
description: "Making outgoing HTTP requests from a Kotlin Golem agent. Use when the user asks to call an external API, make HTTP requests, use fetch, or send HTTP requests from Kotlin agent code."
---

# Making Outgoing HTTP Requests (Kotlin)

## Overview

A Kotlin agent sends outgoing HTTP through `cloud.golem.runtime.wasi.HttpOutgoingClient`, a thin binding over `wasi:http/client@0.3.0`. There is no `fetch`, no Ktor, no OkHttp, no `java.net.http.HttpClient` — a Kotlin agent is a native Wasm component (Kotlin/Wasm, WasmGC), and none of those libraries' transports exist inside it. `HttpOutgoingClient.send` is the only outgoing HTTP path.

`send` is synchronous from the caller's point of view: it fully buffers the request body (if any) and the response body, and returns once the response has arrived. There is no streaming body API in v1.

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.wasi.HttpErrorCode
import cloud.golem.runtime.wasi.HttpHeader
import cloud.golem.runtime.wasi.HttpOutgoingClient
import cloud.golem.runtime.wasi.HttpOutgoingRequest
import cloud.golem.runtime.wasi.HttpOutgoingResponse
```

> `cloud.golem.runtime.wasi.HttpHeader` (outgoing client) and `cloud.golem.http.HttpHeader` (incoming `@HttpHandler`/`@Endpoint` parameters, see `golem-add-http-endpoint-kotlin`) are different types with the same simple name. Alias one if a file needs both, e.g. `import cloud.golem.runtime.wasi.HttpHeader as WasiHttpHeader`.

## Basic GET

```kotlin
fun fetchWeather(city: String): String {
    val request = HttpOutgoingRequest(
        method = "GET",
        url = "https://api.weather.example.com/current?city=$city",
        headers = listOf(HttpHeader("Accept", "application/json")),
    )
    return when (val result = HttpOutgoingClient.send(request)) {
        is Either.Left -> throw RuntimeException("weather lookup failed: ${result.value}")
        is Either.Right -> result.value.body.decodeToString()
    }
}
```

## POST with a Body

`HttpOutgoingRequest.body` is a plain `ByteArray?`. Encode structured payloads yourself (e.g. with `kotlinx.serialization`) before passing them in:

```kotlin
fun postOrder(orderId: String, amount: Long): HttpOutgoingResponse {
    val payload = """{"orderId":"$orderId","amount":$amount}""".encodeToByteArray()
    val request = HttpOutgoingRequest(
        method = "POST",
        url = "https://payments.example.com/charge",
        headers = listOf(HttpHeader("Content-Type", "application/json")),
        body = payload,
    )
    return when (val result = HttpOutgoingClient.send(request)) {
        is Either.Left -> throw RuntimeException("charge failed: ${result.value}")
        is Either.Right -> result.value
    }
}
```

`Content-Length` is computed and added automatically when a body is present and the caller did not already set one. Do not add it yourself unless you need a different value — a body sent without any length header is transferred chunked, and not every server (including simple test doubles) decodes a chunked request body.

## Handling Errors

`HttpOutgoingClient.send` returns `Either<HttpErrorCode, HttpOutgoingResponse>`, never an exception, for anything the WASI HTTP layer itself classifies as a transport-level failure (DNS, TLS, connection refused, timeouts, malformed request/response, size limits, and more — `HttpErrorCode` is a sealed class with one case per `wasi:http/types@0.3.0` `error-code` variant case). A non-2xx HTTP status is **not** one of these — it arrives as an ordinary `Either.Right` with `status` in the 4xx/5xx range; check `response.status` yourself:

```kotlin
when (val result = HttpOutgoingClient.send(request)) {
    is Either.Left -> when (val error = result.value) {
        is HttpErrorCode.ConnectionRefused -> retryLater()
        is HttpErrorCode.DnsTimeout -> retryLater()
        else -> throw RuntimeException("http send failed: $error")
    }
    is Either.Right -> {
        val response = result.value
        if (response.status !in 200..299) {
            throw RuntimeException("unexpected status ${response.status}: ${response.body.decodeToString()}")
        }
        response
    }
}
```

## Retrying on Non-2xx Responses

From Golem's durability point of view, the recorded oplog entry already contains the response the host received, even if its status is `500` — any failure your code derives from that status is a separate, later failure, not part of the HTTP call itself.

Use a host retry policy to retry the HTTP call itself when a selected status arrives, rather than looping in guest code. Load `golem-retry-policies-kotlin` for the full DSL; the policy is matched by the host against properties of the outgoing request/response (`statusCode`, `verb`, `uriHost`, and more), independently of how the SDK issued the call:

```kotlin
import cloud.golem.runtime.host.*

RetryApi.setRetryPolicy(
    NamedPolicy(
        name = "http-5xx-retry",
        policy = Policy.exponential(200.milliseconds, factor = 2.0).maxRetries(3),
        predicate = Props.statusCode.gte(500),
    ),
)
```

> **POST/PUT/PATCH are eligible for retry.** There is no idempotence-mode switch to opt out. Make writes duplicate-safe at the peer: exactly-once processing requires the peer to enforce an idempotency key, since a lost response does not prove the write failed.

Use explicit guest control flow instead when the retry decision depends on decoded response content, not just the status code.

## Calling Golem Agent HTTP Endpoints

When calling another Golem agent's endpoint (or your own), ordinary structured body parameters are deserialized from a **JSON object** with the body parameter names as top-level fields — see `golem-http-params-kotlin`. A single `String` parameter named `body` needs `{"body": "a"}`, not a bare `"a"` string:

```kotlin
// Correct -- JSON object with the field name matching the endpoint's parameter
val request = HttpOutgoingRequest(
    method = "POST",
    url = "http://my-app.localhost:9006/recorder/main/record",
    headers = listOf(HttpHeader("Content-Type", "application/json")),
    body = """{"body": "a"}""".encodeToByteArray(),
)
```

Unstructured text/binary parameters (`UnstructuredText`/`UnstructuredBinary`) instead consume the raw body with no wrapper object — see `golem-http-params-kotlin` for the full set of rules.

## Known Limitation: Large Request Bodies

A multi-MB outgoing request body hangs `client.send` indefinitely — confirmed live, not yet fixed on the host side (tracked against `golem-worker-executor`'s `DurableRequestBody`). This happens the same way whether the body is written in one call or many smaller ones, so chunking the write in guest code does not work around it. Until this is fixed, keep outgoing request bodies to ordinary API-payload sizes (comfortably under a few hundred KB); do not use `HttpOutgoingClient` to upload large files or bulk payloads. The response-body read side was not observed to have this problem.

## Related Skills

- `golem-add-llm-kotlin` — calling an LLM provider's REST API, built on this skill
- `golem-retry-policies-kotlin` — host-level retry policies
- `golem-add-secret-kotlin` — keeping API keys and tokens out of plain config
- `golem-http-params-kotlin` — the JSON body-wrapping rules for calling another Golem agent's endpoint
- `golem-add-http-endpoint-kotlin` — serving incoming HTTP (unrelated mechanism, opposite direction)
