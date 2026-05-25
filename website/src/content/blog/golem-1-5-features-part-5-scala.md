---
title: "Golem 1.5 features — Part 5: Scala support"
date: "2026-04-14T20:45:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-5-scala"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part5-scala/"
---

## Introduction

This post is part of a series showcasing new features of Golem 1.5, releasing at the end of April 2026. The series assumes reader familiarity with Golem. Previous parts covered code-first routes, webhooks, MCP, and Node.js compatibility. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Scala support

Scala gets compiled to JS using Scala.js, and executed in our QuickJS based runtime — rather than directly to WASM. The Golem Scala SDK integrates with the CLI and supports features available in other languages.

### CLI integration

Scala is now selectable when creating new Golem applications. The CLI handles type parsing using Scala syntax and includes a huge catalog of Scala specific agentic skills for AI coding assistants. Building uses the standard `golem build` command with an sbt plugin.

Limitations include no Scala REPL yet and no bridge library generator support.

### Code-first features

Developers can define agents using Scala annotations:

```scala
@agentDefinition(mount = "/counters/{name}")
trait CounterAgent extends BaseAgent {

  class Id(val name: String)

  @prompt("Increase the count by one")
  @description("Increases the count by one and returns the new value")
  @endpoint(method = "POST", path = "/increment")
  def increment(): Future[Int]
}

@agentImplementation()
final class CounterAgentImpl(@unused private val name: String) extends CounterAgent {
  private var count: Int = 0

  override def increment(): Future[Int] = Future.successful {
    count += 1
    count
  }
}
```

### RPC

Client classes are automatically generated for remote agent invocation:

```scala
@agentDefinition()
@description("Calls CounterAgent remotely and returns the result.")
trait CallerAgent extends BaseAgent {
  class Id(val value: String)

  @description("Increments the given counter N times and returns the final value.")
  def incrementCounter(counterId: String, times: Int): Future[Int]
}

@agentImplementation()
final class CallerAgentImpl(@unused private val name: String) extends CallerAgent {
  override def incrementCounter(counterId: String, times: Int): Future[Int] = {
    val counter = CounterAgentClient.get(counterId)

    (1 until times)
      .foldLeft(counter.increment()) { (prev, _) =>
        prev.flatMap(_ => counter.increment())
      }
  }
}
```

### HTTP

The post demonstrates using `zio-http` with Scala.js's fetch backend for HTTP requests:

```scala
@agentImplementation()
final class FetchAgentImpl() extends FetchAgent {
  override def fetchFromPort(port: Int): Future[String] = {
    val effect =
      for {
        response <- ZIO.serviceWithZIO[Client] { client =>
                      client.url(url"http://localhost").port(port).batched.get("/test")
                    }
        body <- response.body.asString
      } yield body;

    Unsafe.unsafe { implicit u =>
      Runtime.default.unsafe.runToFuture(effect.provide(ZClient.default))
    }
  }
}
```

## Next steps

This foundation enables building higher-level Scala integrations, leveraging ZIO Schema for improved developer experience.
