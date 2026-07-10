---
name: golem-call-from-external-scala
description: "Calling Golem agents from external applications when the agent is written in Scala. Use when the user wants to invoke Scala agents from outside the Golem platform."
---

# Calling Agents from External Applications (Scala)

## Generate a Scala Bridge SDK

Golem can generate typed Scala bridge SDKs for calling agents from external JVM applications. The bridge target language is independent of the agent's source language: a Scala bridge can call agents written in Rust, TypeScript, Scala, or MoonBit.

Configure bridge generation in `golem.yaml`:

```yaml
bridge:
  scala:
    external:
      agents: "*"
```

Or generate a bridge directly:

```shell
golem generate-bridge --language scala --agent-type-name MyAgent --output-dir scala-bridge
```

The generated output is a self-contained sbt project. For an agent type named `CounterAgent`, the client package is `golem.bridge.client.counter_agent` and the generated client object is `CounterAgentClient`.

### Setup

Use the generated sbt project as a dependency from your external Scala application, or add your application entry point inside the generated project while prototyping.

```scala
// build.sbt
scalaVersion := "3.8.2"
name := "external-client"

lazy val bridge = RootProject(file("../scala-bridge/counter-agent-client"))

lazy val root = (project in file("."))
  .dependsOn(bridge)
```

### Example

```scala
import golem.bridge.client.counter_agent.CounterAgentClient
import golem.bridge.runtime.GolemServer

import scala.concurrent.Await
import scala.concurrent.duration._

@main def main(): Unit =
  CounterAgentClient.configure(GolemServer.Local, "my-app", "local")

  val timeout = 30.seconds
  val counter = Await.result(CounterAgentClient.get("my-counter"), timeout)
  val result  = Await.result(counter.increment(), timeout)

  println(result)
```

### Building and Running

```shell
sbt run
```

### Authentication

- **Local server**: Use `GolemServer.Local`.
- **Golem Cloud**: Use `GolemServer.Cloud(token)` with your API token.
- **Custom deployment**: Use `GolemServer.Custom(url, token)`.

### Agent Constructors and Methods

Generated Scala bridge clients follow the same conventions as agent-to-agent RPC:

- `Client.get(...)` attaches to an existing agent instance.
- `Client.getPhantom(...)` attaches to an existing phantom instance.
- `Client.newPhantom(...)` creates a new phantom instance.
- Methods returning a result are exposed as `Future[T]`.
- Fire-and-forget and scheduled methods return `Future[Unit]`.

## Using a Generated TypeScript or Rust Bridge

You can also generate a **TypeScript** or **Rust** bridge SDK for agents written in Scala. The bridge target language is independent of the agent's source language:

```yaml
bridge:
  ts:
    external:
      agents: "*"
  rust:
    external:
      agents: "*"
```

Then use the generated TypeScript or Rust client from your external application. See the `golem-call-from-external-ts` or `golem-call-from-external-rust` skills (available in TypeScript and Rust project templates) for details.
