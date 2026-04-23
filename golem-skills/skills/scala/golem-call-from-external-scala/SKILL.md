---
name: golem-call-from-external-scala
description: "Calling Golem agents from external applications when the agent is written in Scala. Use when the user wants to invoke Scala agents from outside the Golem platform."
---

# Calling Agents from External Applications (Scala)

## Bridge Generation Not Yet Available

Golem's bridge SDK generation currently supports **TypeScript** and **Rust** target languages only. There is no Scala bridge generator yet.

## Alternative: Call the REST API from Scala

You can call any Golem agent using the REST API from a standalone Scala (JVM) application. Use Java's built-in `java.net.http.HttpClient` — no additional dependencies needed.

### Setup

Create a separate Scala project (outside the Golem component) for the external CLI app. This is a regular JVM Scala project (not Scala.js):

```scala
// build.sbt
scalaVersion := "3.6.4"
name := "external-client"
```

### Example

```scala
import java.net.URI
import java.net.http.{HttpClient, HttpRequest, HttpResponse}

@main def main(): Unit =
  val client = HttpClient.newHttpClient()
  val token  = "5c832d93-ff85-4a8f-9803-513950fdfdb1"  // local well-known token

  def invokeAgent(
    appName: String, envName: String,
    agentTypeName: String, agentName: String,
    methodName: String
  ): String =
    val body = s"""{
      "appName": "$appName",
      "envName": "$envName",
      "agentTypeName": "$agentTypeName",
      "parameters": { "type": "Tuple", "elements": [{ "type": "ComponentModel", "value": "$agentName" }] },
      "methodName": "$methodName",
      "methodParameters": { "type": "Tuple", "elements": [] },
      "mode": "await"
    }"""
    val request = HttpRequest.newBuilder()
      .uri(URI.create("http://localhost:9881/v1/agents/invoke-agent"))
      .header("Content-Type", "application/json")
      .header("Authorization", s"Bearer $token")
      .POST(HttpRequest.BodyPublishers.ofString(body))
      .build()
    client.send(request, HttpResponse.BodyHandlers.ofString()).body()

  val result = invokeAgent("my-app", "local", "MyAgent", "my-instance", "doSomething")
  println(result)
```

### Building and Running

```shell
sbt run
```

### REST API Reference

**Endpoint**: `POST /v1/agents/invoke-agent`

**Request body**:

```json
{
  "appName": "my-app",
  "envName": "local",
  "agentTypeName": "MyAgent",
  "parameters": {
    "type": "Tuple",
    "elements": [
      { "type": "ComponentModel", "value": "my-instance" }
    ]
  },
  "methodName": "doSomething",
  "methodParameters": {
    "type": "Tuple",
    "elements": [
      { "type": "ComponentModel", "value": "input" }
    ]
  },
  "mode": "await"
}
```

**Response body** (when mode is `"await"`):

```json
{
  "result": {
    "type": "Tuple",
    "elements": [
      { "type": "ComponentModel", "value": <result_value> }
    ]
  }
}
```

### Authentication

- **Local server**: Use bearer token `5c832d93-ff85-4a8f-9803-513950fdfdb1`
- **Golem Cloud**: Use your API token
- **Custom deployment**: Use the configured bearer token

## Using a Generated TypeScript or Rust Bridge

If you need a typed client, you can generate a **TypeScript** or **Rust** bridge SDK even for agents written in Scala. The bridge target language is independent of the agent's source language:

```yaml
bridge:
  ts:
    agents: "*"
  rust:
    agents: "*"
```

Then use the generated TypeScript or Rust client from your external application. See the `golem-call-from-external-ts` or `golem-call-from-external-rust` skills (available in TypeScript and Rust project templates) for details.
