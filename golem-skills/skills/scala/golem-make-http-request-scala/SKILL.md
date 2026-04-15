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

When making HTTP requests to other Golem agent endpoints (or your own), the request body must match the **Golem HTTP body mapping convention**: non-binary body parameters are always deserialized from a **JSON object** where each top-level field corresponds to a method parameter name. This is true even when the endpoint has a single body parameter.

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

> **Rule of thumb:** If the target endpoint is a Golem agent, always send `application/json` with parameter names as JSON keys. Load the `golem-http-params-scala` skill for the full body mapping rules.

## Key Constraints

- Golem Scala apps are compiled to JavaScript via **Scala.js** — the `fetch` API is available as a global function
- For simple requests, use `js.Dynamic.global.fetch` directly
- For ZIO-based agents, use `zio-http` which provides a typed Scala API
- Third-party JVM HTTP clients (Apache HttpClient, OkHttp, sttp with non-JS backends) will **NOT** work — they depend on JVM networking APIs
- Libraries must be Scala.js-compatible (use `%%%` in `build.sbt`)
- All HTTP requests go through the WASI HTTP layer under the hood
