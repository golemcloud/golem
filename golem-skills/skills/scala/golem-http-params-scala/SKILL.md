---
name: golem-http-params-scala
description: "Mapping HTTP request elements to Scala agent parameters. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response type mapping for HTTP endpoints."
---

# HTTP Request and Response Parameter Mapping (Scala)

## Overview

When an agent is exposed over HTTP, Golem maps parts of each HTTP request to constructor and method parameters. This skill covers how path segments, query parameters, headers, and request bodies are mapped, which types are supported for each, and how return types map to HTTP responses.

## Path Variables

Path variables `{varName}` in mount or endpoint paths map to parameters by name:

```scala
// Mount path variables → constructor parameters (from class Id)
@agentDefinition(mount = "/api/tasks/{name}")
trait TaskAgent extends BaseAgent {
  class Id(val name: String)

  // Endpoint path variables → method parameters
  @endpoint(method = "GET", path = "/items/{itemId}")
  def getItem(itemId: String): Future[Item]
}
```

Remaining (catch-all) path variables capture everything after a prefix:

```scala
@endpoint(method = "GET", path = "/files/{*filePath}")
def getFile(filePath: String): Future[String]
// GET .../files/docs/readme.md → filePath = "docs/readme.md"
```

Catch-all variables can only appear as the **last** path segment and are **not** allowed in mount paths.

## Query Parameters

Specified in the endpoint path using `?key={var}` syntax:

```scala
@endpoint(method = "GET", path = "/search?q={query}&limit={n}")
def search(query: String, n: Int): Future[String]
// GET .../search?q=hello&limit=10
```

## Header Variables

Map HTTP headers to parameters using the `@header` annotation on individual parameters:

```scala
import golem.runtime.annotations.header

@endpoint(method = "POST", path = "/report")
def submitReport(
  @header("X-Tenant") tenantId: String,
  data: String
): Future[String]
```

## Supported Types for Path, Query, and Header Variables

Only these types can be used for parameters bound to path/query/header variables (the value is parsed from the URL/header string):

| Scala Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `Boolean` | Parsed from `"true"` / `"false"` |
| `Int` | Parsed as 32-bit signed integer |
| `Long` | Parsed as 64-bit signed integer |
| `Float` | Parsed as 32-bit float |
| `Double` | Parsed as 64-bit float |
| Enum / sealed trait (unit cases only) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper types are supported:

| Scala Type | Behavior |
|---|---|
| `Option[T]` (where `T` is a supported type above) | Optional — absent query param or header produces `None` |
| `List[T]` / `Array[T]` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (case classes, tuples, sealed traits with data, etc.) can only be used as **body parameters**.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters **not** bound to path variables, query parameters, or headers are populated from the JSON request body:

```scala
@endpoint(method = "POST", path = "/items/{id}")
def updateItem(id: String, name: String, count: Int): Future[Item]
// POST .../items/123
// Body: { "name": "Widget", "count": 5 }
// → id from path, name and count from body
```

Each unmapped parameter becomes a top-level field in the expected JSON body object. All custom types require a `zio.blocks.schema.Schema` instance.

> **⚠️ Important for callers:** When making HTTP requests *to* a Golem agent endpoint, always send a JSON object with the parameter names as keys — even for a single `String` body parameter. For example, calling the `updateItem` endpoint above requires `{"name": "Widget", "count": 5}`, **not** a raw text string. See the `golem-make-http-request-scala` skill for examples.

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `Future[Unit]` | 204 No Content | empty |
| `Future[T]` | 200 OK | JSON-serialized `T` |
| `Future[Option[T]]` | 200 OK if `Some`, 404 Not Found if `None` | JSON `T` or empty |

## Data Type to JSON Mapping

| Scala Type | JSON Representation |
|---|---|
| `String` | JSON string |
| `Int`, `Long` | JSON number (integer) |
| `Float`, `Double` | JSON number (float) |
| `Boolean` | JSON boolean |
| `Array[T]`, `List[T]` | JSON array |
| case class (with `Schema`) | JSON object (camelCase field names) |
| `Option[T]` | value or `null` |
| sealed trait / enum | JSON variant |
| `Tuple` | JSON array |
