---
name: golem-add-llm-scala
description: "Adding LLM and AI capabilities to a Scala Golem agent. Use when the user wants to add LLM chat, embeddings, or any AI provider integration to a Scala agent."
---

# Adding LLM and AI Capabilities (Scala)

## Overview

There are no Golem-specific AI libraries for Scala. To integrate with LLM providers, you have two options:

1. **Use a third-party Scala library** — but only if it is **Scala.js-compatible** and uses `fetch` under the hood (JVM-only HTTP clients will not work)
2. **Call the provider's REST API directly** using `fetch` or ZIO HTTP's fetch backend (recommended)

Load the `golem-make-http-request-scala` skill for full details on making HTTP requests from Scala agents.

## Option 1: Using a Third-Party Library

If you find a Scala.js-compatible LLM client library, add it with `%%%` in `build.sbt`:

```scala
libraryDependencies += "com.example" %%% "llm-client" % "1.0.0"
```

> **⚠️ Important:** The library must be published for Scala.js (`_sjs1_` artifact) and must use `fetch` or another browser-compatible HTTP mechanism. Libraries that depend on JVM networking (Apache HttpClient, OkHttp, sttp with JVM backends, etc.) will **not** work.

## Option 2: Calling the API Directly with `fetch` (Recommended)

Since Golem Scala agents compile to JavaScript via Scala.js, the global `fetch` function is available. This is the most reliable approach:

```scala
import scala.scalajs.js
import scala.scalajs.js.Thenable.Implicits._
import scala.concurrent.Future
import scala.concurrent.ExecutionContext.Implicits.global

def chatCompletion(prompt: String, apiKey: String): Future[String] = {
  val payload = js.JSON.stringify(js.Dynamic.literal(
    model = "gpt-4o",
    messages = js.Array(
      js.Dynamic.literal(role = "user", content = prompt)
    )
  ))

  val options = js.Dynamic.literal(
    method = "POST",
    headers = js.Dynamic.literal(
      "Content-Type" -> "application/json",
      "Authorization" -> s"Bearer $apiKey"
    ),
    body = payload
  )

  for {
    response <- js.Dynamic.global.fetch(
                  "https://api.openai.com/v1/chat/completions",
                  options
                ).asInstanceOf[js.Promise[js.Dynamic]].toFuture
    text     <- response.text().asInstanceOf[js.Promise[String]].toFuture
  } yield {
    val json = js.JSON.parse(text)
    json.choices.asInstanceOf[js.Array[js.Dynamic]](0)
      .message.content.asInstanceOf[String]
  }
}
```

## Option 3: Using ZIO HTTP

For ZIO-based agents, use `zio-http` which provides a typed Scala HTTP client:

```scala
import zio._
import zio.http._
import scala.concurrent.Future

def chatCompletion(prompt: String, apiKey: String): Future[String] = {
  val body = Body.fromString(
    s"""{"model": "gpt-4o", "messages": [{"role": "user", "content": "$prompt"}]}"""
  )

  val effect =
    (for {
      response <- ZIO.serviceWithZIO[Client] { client =>
                    client
                      .url(URL.decode("https://api.openai.com").toOption.get)
                      .addHeader(Header.ContentType(MediaType.application.json))
                      .addHeader(Header.Authorization.Bearer(apiKey))
                      .batched
                      .post("/v1/chat/completions")(body)
                  }
      text <- response.body.asString
    } yield {
      // Parse the JSON response to extract the message content
      text
    }).provide(ZClient.default)

  Unsafe.unsafe { implicit u =>
    Runtime.default.unsafe.runToFuture(effect)
  }
}
```

## Setting API Keys

Store provider API keys as **secrets** using Golem's typed config system. Load the `golem-add-secret-scala` skill for full details. In brief, declare the key in your config case class:

```scala
import golem.config.{Config, Secret}
import zio.blocks.schema.Schema

final case class MyAgentConfig(apiKey: Secret[String])
object MyAgentConfig {
  implicit val schema: Schema[MyAgentConfig] = Schema.derived
}
```

Then manage it via the CLI:

```shell
golem agent-secret create apiKey --secret-type string --secret-value "sk-..."
```

Access in code with `config.value.apiKey.get`.

## Complete Agent Example

```scala
import golem.runtime.annotations.{agentDefinition, agentImplementation, endpoint}
import golem.BaseAgent
import scala.scalajs.js
import scala.scalajs.js.Thenable.Implicits._
import scala.concurrent.Future
import scala.concurrent.ExecutionContext.Implicits.global

@agentDefinition(mount = "/chats/{value}")
trait ChatAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "POST", path = "/ask")
  def ask(question: String): Future[String]
}

@agentImplementation()
final class ChatAgentImpl(private val chatName: String) extends ChatAgent {
  private var messages: List[js.Dynamic] = List(
    js.Dynamic.literal(
      role = "system",
      content = s"You are a helpful assistant for chat '$chatName'"
    )
  )

  override def ask(question: String): Future[String] = {
    messages = messages :+ js.Dynamic.literal(role = "user", content = question)

    val apiKey = sys.env.getOrElse("OPENAI_API_KEY",
      throw new RuntimeException("OPENAI_API_KEY not set"))

    val payload = js.JSON.stringify(js.Dynamic.literal(
      model = sys.env.getOrElse("LLM_MODEL", "gpt-4o"),
      messages = js.Array(messages: _*)
    ))

    val options = js.Dynamic.literal(
      method = "POST",
      headers = js.Dynamic.literal(
        "Content-Type" -> "application/json",
        "Authorization" -> s"Bearer $apiKey"
      ),
      body = payload
    )

    for {
      response <- js.Dynamic.global.fetch(
                    "https://api.openai.com/v1/chat/completions",
                    options
                  ).asInstanceOf[js.Promise[js.Dynamic]].toFuture
      text     <- response.text().asInstanceOf[js.Promise[String]].toFuture
    } yield {
      val json = js.JSON.parse(text)
      val reply = json.choices.asInstanceOf[js.Array[js.Dynamic]](0)
        .message.content.asInstanceOf[String]
      messages = messages :+ js.Dynamic.literal(role = "assistant", content = reply)
      reply
    }
  }
}
```

## Key Constraints

- Golem Scala agents are compiled to JavaScript via **Scala.js** — only Scala.js-compatible libraries work
- Third-party libraries must use `fetch` or another browser-compatible HTTP mechanism — JVM HTTP clients will **not** work
- Use `%%%` (not `%%`) in `build.sbt` for Scala.js-compatible dependencies
- Calling the REST API directly with `fetch` is the most reliable approach
- API keys should be stored as secrets using Golem's typed config system (load the `golem-add-secret-scala` skill)
- All HTTP requests are automatically durably persisted by Golem
