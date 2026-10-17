---
name: golem-make-http-request-scala
description: "Making outgoing HTTP requests from a Scala Golem agent. Use when the user asks to call an external API, make HTTP requests, use fetch, or send HTTP requests from agent code."
---

# Making Outgoing HTTP Requests (Scala)

## Overview

Golem Scala agents are compiled to JavaScript via Scala.js and run in a QuickJS-based WASM runtime. The recommended way to make HTTP requests is using the standard **`fetch` API** via Scala.js interop, or using **ZIO HTTP** which internally uses the same WASI HTTP layer.

## Option 1: Using `fetch` via Scala.js (Recommended for Simple Requests)

Since Golem Scala apps compile to JavaScript, the global `fetch` function is available:

```scala
import scala.scalajs.js
import scala.scalajs.js.Thenable.Implicits._
import scala.concurrent.Future
import scala.concurrent.ExecutionContext.Implicits.global

def fetchData(url: String): Future[String] = {
  val options = js.Dynamic.literal(
    method = "GET",
    headers = js.Dynamic.literal(
      "Accept" -> "application/json"
    )
  )

  for {
    response <- js.Dynamic.global.fetch(url, options)
                  .asInstanceOf[js.Promise[js.Dynamic]].toFuture
    text     <- response.text().asInstanceOf[js.Promise[String]].toFuture
  } yield text
}
```

### POST with JSON Body

```scala
def postData(url: String, payload: String): Future[String] = {
  val options = js.Dynamic.literal(
    method = "POST",
    headers = js.Dynamic.literal(
      "Content-Type" -> "application/json",
      "Accept" -> "application/json"
    ),
    body = payload
  )

  for {
    response <- js.Dynamic.global.fetch(url, options)
                  .asInstanceOf[js.Promise[js.Dynamic]].toFuture
    text     <- response.text().asInstanceOf[js.Promise[String]].toFuture
  } yield text
}
```

## Option 2: Using ZIO HTTP (Recommended for ZIO-Based Agents)

If you already use ZIO in your agent, `zio-http` provides a typed Scala HTTP client:

```scala
import zio._
import zio.http._
import scala.concurrent.Future

def fetchFromUrl(url: String): Future[String] = {
  val effect =
    (for {
      response <- ZIO.serviceWithZIO[Client] { client =>
                    client.url(URL.decode(url).toOption.get).batched.get("/")
                  }
      body <- response.body.asString
    } yield body).provide(ZClient.default)

  Unsafe.unsafe { implicit u =>
    Runtime.default.unsafe.runToFuture(effect)
  }
}
```

### ZIO HTTP POST Example

```scala
import zio._
import zio.http._
import scala.concurrent.Future

def postJson(url: String, jsonBody: String): Future[String] = {
  val effect =
    (for {
      response <- ZIO.serviceWithZIO[Client] { client =>
                    client
                      .url(URL.decode(url).toOption.get)
                      .addHeader(Header.ContentType(MediaType.application.json))
                      .batched
                      .post("/")(Body.fromString(jsonBody))
                  }
      body <- response.body.asString
    } yield body).provide(ZClient.default)

  Unsafe.unsafe { implicit u =>
    Runtime.default.unsafe.runToFuture(effect)
  }
}
```

> **Note:** ZIO HTTP requires `zio-http` as a Scala.js-compatible dependency in your `build.sbt`.

## Retrying on non-2xx Responses

Both `fetch` and ZIO HTTP resolve successfully when the server returns an HTTP response, even if
the status is `500`. From Golem's durability point of view, the recorded oplog entry already
contains the `500` response, and any subsequent application-level failure derived from the
status is a separate failure *after* the side effect.

Use a host retry policy to retry the HTTP call itself when a selected status arrives.

### Preferred: a `status-code`-keyed retry policy

Define a named retry policy whose predicate explicitly references `status-code`. When the
response arrives the host transparently re-sends the request without an application-level throw.
The resolved response is the *last* attempt. See the
`golem-retry-policies-scala` skill for the policy YAML / SDK shape.

> **POST/PUT/PATCH are eligible for retry.** The guest cannot opt out with an idempotence-mode
> switch. Make writes duplicate-safe at the peer: exactly-once processing requires the peer to
> enforce an idempotency key. A lost response does not prove the write failed. Do not predicate
> retry policies on `is-idempotent`; the host no longer supplies that property.

Minimal POST example — the policy is defined once (in `golem.yaml` or via the SDK) and the call
site is just a plain HTTP call:

```yaml
# golem.yaml — under retryPolicyDefaults / <env>:
http-5xx-retry:
  priority: 20
  predicate:
    propIn: { property: "status-code", values: [500, 502, 503, 504] }
  policy:
    countBox:
      maxRetries: 3
      inner:
        exponential:
          baseDelay: "200ms"
          factor: 2.0
```

```scala
// The host applies the configured status-code policy.
val options = js.Dynamic.literal(
  method = "POST",
  headers = js.Dynamic.literal("Content-Type" -> "application/json"),
  body    = js.JSON.stringify(js.Dynamic.literal(orderId = orderId, amount = amount))
)
for {
  response <- js.Dynamic.global.fetch("https://payments.example.com/charge", options)
                .asInstanceOf[js.Promise[js.Dynamic]].toFuture
  // The host already retried up to maxRetries times against the 5xx policy;
  // `response` is the *final* response after retries were exhausted.
  ok        = response.ok.asInstanceOf[Boolean]
  charge   <- if (ok) response.json().asInstanceOf[js.Promise[js.Dynamic]].toFuture
              else Future.failed(new RuntimeException(s"charge failed: ${response.status}"))
} yield charge
```

If status retry "doesn't seem to work", load the `golem-local-dev-server` skill and look for the
`HTTP status retry skipped, reason: …` debug line — it pinpoints why a particular request was
not retried (e.g. `BodyNotFinished`, `NotIdempotent`, `NoRetry`).

### Application-level retry

Use explicit guest control flow or an SDK saga when the decision depends on decoded content.
Each new attempt appends new calls and retains earlier responses. Throwing after a response
fails the invocation without retrying the completed request. Host transport and body-resumption
retries remain host responsibilities.

## Complete Example in an Agent

```scala
import golem.runtime.annotations.{agentDefinition, agentImplementation, endpoint}
import golem.BaseAgent
import scala.scalajs.js
import scala.scalajs.js.Thenable.Implicits._
import scala.concurrent.Future
import scala.concurrent.ExecutionContext.Implicits.global

@agentDefinition(mount = "/weather/{value}")
trait WeatherAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/current")
  def getCurrent(): Future[String]
}

@agentImplementation()
final class WeatherAgentImpl(private val city: String) extends WeatherAgent {

  override def getCurrent(): Future[String] = {
    val url = s"https://api.weather.example.com/current?city=$city"

    val options = js.Dynamic.literal(
      method = "GET",
      headers = js.Dynamic.literal("Accept" -> "application/json")
    )

    for {
      response <- js.Dynamic.global.fetch(url, options)
                    .asInstanceOf[js.Promise[js.Dynamic]].toFuture
      text     <- response.text().asInstanceOf[js.Promise[String]].toFuture
    } yield text
  }
}
```

## Calling Golem Agent HTTP Endpoints

When making HTTP requests to other Golem agent endpoints (or your own), ordinary structured body parameters are deserialized from a **JSON object** with body parameter names as top-level fields. Include only parameters not bound to path/query/header values. A single object parameter named `payload` needs `{"payload": {"name": "Widget", "count": 5}}`, not the bare inner object. Unstructured text and binary parameters instead consume the raw body without this wrapper.

For example, given this endpoint definition:

```scala
@endpoint(method = "POST", path = "/record")
def record(body: String): Future[Unit]
```

The correct HTTP request must send a JSON object with a `body` field — **not** a raw text string:

```scala
// ✅ CORRECT — JSON object with field name matching the parameter
val options = js.Dynamic.literal(
  method = "POST",
  headers = js.Dynamic.literal("Content-Type" -> "application/json"),
  body = """{"body": "a"}"""
)
js.Dynamic.global.fetch("http://my-app.localhost:9006/recorder/main/record", options)
  .asInstanceOf[js.Promise[js.Dynamic]].toFuture

// ❌ WRONG — raw text body does NOT match Golem's JSON body mapping
val options = js.Dynamic.literal(
  method = "POST",
  headers = js.Dynamic.literal("Content-Type" -> "text/plain"),
  body = "a"
)
```

> **Rule of thumb:** For structured Golem agent bodies, send `application/json` with body parameter names as JSON keys. For unstructured text, send raw UTF-8 text with `text/plain`; for unstructured binary, send raw bytes with the appropriate MIME type. Load `golem-http-params-scala` for the restrictions and the separate CLI positional/native-literal rules; do not reuse the HTTP wrapper for CLI arguments.

## Key Constraints

- Golem Scala apps are compiled to JavaScript via **Scala.js** — the `fetch` API is available as a global function
- For simple requests, use `js.Dynamic.global.fetch` directly
- For ZIO-based agents, use `zio-http` which provides a typed Scala API
- Third-party JVM HTTP clients (Apache HttpClient, OkHttp, sttp with non-JS backends) will **NOT** work — they depend on JVM networking APIs
- Libraries must be Scala.js-compatible (use `%%%` in `build.sbt`)
- All HTTP requests go through the WASI HTTP layer under the hood
