---
name: golem-http-params-kotlin
description: "Mapping HTTP request elements to Kotlin agent parameters. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response type mapping for HTTP endpoints."
---

# HTTP Request and Response Parameter Mapping (Kotlin)

## Overview

When an agent is exposed over HTTP, Golem maps parts of each HTTP request to constructor and method parameters. This skill covers how path segments and HTTP verb+path are mapped, which types are supported, and how return types map to HTTP responses.

The HTTP gateway runs on **port 9006** in local development (`golem server run`).

## Mount Path and Constructor Parameters

The `mount` attribute of `@Agent` defines the URL prefix for all of this agent's endpoints. Path variables `{varName}` in the mount path bind to constructor parameters by name:

```kotlin
@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {
    // All endpoints for CounterAgent are rooted at /counters/{name}
}
```

A `POST /counters/my-counter/increment` request creates (or retrieves) `CounterAgent("my-counter")` and calls its `increment` method.

## Endpoint Verb and Sub-Path

The `@Endpoint` annotation maps a method to an HTTP verb and sub-path relative to the mount:

```kotlin
@Agent(mount = "/api/tasks/{taskId}", description = "Task management agent")
class TaskAgent(val taskId: String) : BaseAgent() {

    @Endpoint(get = "/status")
    fun getStatus(): String = TODO()
    // GET /api/tasks/{taskId}/status

    @Endpoint(post = "/complete")
    fun complete(reason: String): Boolean = TODO()
    // POST /api/tasks/{taskId}/complete

    @Endpoint(put = "/rename")
    fun rename(newName: String): Unit = TODO()
    // PUT /api/tasks/{taskId}/rename

    @Endpoint(delete = "/cancel")
    fun cancel(): Unit = TODO()
    // DELETE /api/tasks/{taskId}/cancel
}
```

The verb attribute (`post`, `get`, `put`, `delete`) holds the sub-path. Only one verb per `@Endpoint` is used; `path` is an alternative for specifying the sub-path without implying a verb (used when the HTTP method is inferred by convention).

## Full Counter Example

```kotlin
package counter

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {

    private var value: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        value++
        return value
    }

    @Endpoint(get = "/value")
    fun getValue(): Int = value

    @Endpoint(get = "/whoami")
    fun whoAmI(): String = agentId
}
```

With the HTTP gateway running locally, this exposes:

| HTTP Request | Agent call |
|---|---|
| `POST http://localhost:9006/counters/my-counter/increment` | `CounterAgent("my-counter").increment()` |
| `GET  http://localhost:9006/counters/my-counter/value` | `CounterAgent("my-counter").getValue()` |
| `GET  http://localhost:9006/counters/my-counter/whoami` | `CounterAgent("my-counter").whoAmI()` |

## Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters are populated from the JSON request body. The body must be a JSON object with parameter names as keys:

```kotlin
@Endpoint(post = "/process")
fun process(input: String, count: Int): String = TODO()
// POST body: { "input": "hello", "count": 3 }
```

> **Important:** The request body is **always** a JSON object with parameter names as keys — even when there is only a single body parameter. Sending a bare string or non-object JSON will fail.

## Supported Parameter Types

Parameters bound to path variables must be one of these primitive types:

| Kotlin Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `Boolean` | Parsed from `"true"` / `"false"` |
| `Int` | Parsed as 32-bit signed integer |
| `Long` | Parsed as 64-bit signed integer |
| `Float` | Parsed as 32-bit float |
| `Double` | Parsed as 64-bit float |

Complex types (data classes, lists, etc.) can only be used as **body parameters** (on POST/PUT/DELETE endpoints).

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `Unit` | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |

> **Note:** Header-variable binding (`@header` annotation) and query-parameter syntax (`?key={var}`) are planned for a future SDK phase and are not yet available in the Kotlin SDK.
