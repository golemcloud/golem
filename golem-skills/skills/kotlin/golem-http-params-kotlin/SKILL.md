---
name: golem-http-params-kotlin
description: "Mapping HTTP request elements to Kotlin agent parameters. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response type mapping for HTTP endpoints."
---

# HTTP Request and Response Parameter Mapping (Kotlin)

## Overview

When an agent is exposed over HTTP (see `golem-add-http-endpoint-kotlin` for mounting and routing), Golem maps parts of each HTTP request to constructor and method parameters. This skill covers how path segments, query parameters, headers, and request bodies are mapped, which types are supported for each, and how return types map to HTTP responses.

## Path Variables

Path variables `{varName}` in a mount or endpoint path map to parameters by name:

```kotlin
@Agent(mount = "/api/tasks/{name}")       // mount path variable -> constructor parameter
class TaskAgent(val name: String) : BaseAgent() {
    @Endpoint(get = "/items/{itemId}")     // endpoint path variable -> method parameter
    fun getItem(itemId: String): Item = TODO()
}
```

A catch-all path variable captures everything after a prefix, and can only appear as the **last** segment of an **endpoint** path (never a mount path):

```kotlin
@Endpoint(get = "/files/{+rest}")
fun getFile(rest: String): String = TODO()
// GET .../files/docs/readme.md -> rest = "docs/readme.md"
```

## Query Parameters

Specified in the endpoint path using `?key={var}` syntax:

```kotlin
@Endpoint(get = "/search?q={query}&limit={n}")
fun search(query: String, n: Int): String = TODO()
// GET .../search?q=hello&limit=10
```

## Header Variables

Map HTTP headers to parameters with `@Header` on individual parameters:

```kotlin
import cloud.golem.annotations.Header

@Endpoint(post = "/report")
fun submitReport(@Header("X-Tenant") tenantId: String, data: String): String = TODO()
```

## Supported Types for Path, Query, and Header Variables

Only these types can be used for parameters bound to path/query/header variables (the value is parsed from the URL/header string):

| Kotlin Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `Boolean` | Parsed from `"true"` / `"false"` |
| `Int` | Parsed as 32-bit signed integer |
| `Long` | Parsed as 64-bit signed integer |
| `Float` | Parsed as 32-bit float |
| `Double` | Parsed as 64-bit double |
| `enum class` (no payload) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper forms are supported:

| Kotlin Type | Behavior |
|---|---|
| `T?` (where `T` is a supported type above) | Optional — absent query param or header produces `null` |
| `List<T>` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (data classes, sealed classes, tuples, etc.) can only be used as **body parameters**.

## POST Request Body Mapping

User-supplied method parameters **not** bound to path variables, query parameters, or headers are body parameters. Ordinary structured parameters are populated from the JSON request body:

```kotlin
@Endpoint(post = "/items/{id}")
fun updateItem(id: String, name: String, count: Int): Item = TODO()
// POST .../items/123
// Body: { "name": "Widget", "count": 5 }
// -> id from path, name and count from body
```

Each ordinary structured body parameter becomes a top-level field in the expected JSON body object, with KSP-derived schemas (see `golem-add-agent-kotlin`'s "Custom types").

> **⚠️ Important:** Ordinary structured bodies use a JSON object with **body parameter names only** as keys, even for one parameter. `fun decide(decision: String)` expects `{"decision": "approved"}`, not a bare string. A single object parameter named `payload` also needs wrapping: `{"payload": {"name": "Widget", "count": 5}}`, not `{"name": "Widget", "count": 5}`. Do not include path/query/header parameters. `UnstructuredText`/`UnstructuredBinary` parameters instead consume the raw body; see below.

CLI positional invocation is different: pass one argument per user-supplied method parameter, in signature order, including parameters bound to path/query/header values over HTTP. Use Kotlin value syntax for each argument, not one JSON object wrapping the full input. Load `golem-invoke-agent-kotlin` for CLI examples and caller-only input rules.

## Raw Text and Binary Bodies

Use `cloud.golem.schema.UnstructuredText` for raw `text/plain` and `cloud.golem.schema.UnstructuredBinary` for raw binary request and response bodies. Restrict the allow-list on the parameter's type with `@Languages("en", "de")` / `@MimeTypes("image/png", "image/jpeg")`; without one, every language or MIME type (or none) is accepted.

```kotlin
import cloud.golem.annotations.MimeTypes
import cloud.golem.schema.UnstructuredBinary
import cloud.golem.schema.UnstructuredText

@Endpoint(post = "/upload/{bucket}")
fun upload(bucket: String, @MimeTypes("image/png", "image/jpeg") payload: UnstructuredBinary): Long =
    when (payload) {
        is UnstructuredBinary.Inline -> payload.bytes.size.toLong()
        is UnstructuredBinary.Url -> -1L
    }

@Endpoint(get = "/notes/{id}")
fun getNote(id: String): UnstructuredText = UnstructuredText.fromInline("hello", "en")
```

- Values: `UnstructuredText.Inline(text, languageCode)` / `UnstructuredText.Url(url)` and `UnstructuredBinary.Inline(bytes, mimeType)` / `UnstructuredBinary.Url(url)`; build them with `fromInline(...)` (optionally with a language code / MIME type) or `fromUrl(url)`.
- A method with an unstructured body parameter may have only **one** body parameter, and it cannot be bound to a path/query/header variable.
- Text request bodies must have no `Content-Type`, `text/plain`, or `text/plain; charset=utf-8` (otherwise 415); an optional single `Content-Language` is checked against the allow-list.
- Responses: text is sent as `text/plain; charset=utf-8` with the language code as `Content-Language`; binary uses its MIME type (default `application/octet-stream`); a `Url` value is a 307 redirect.

## Return Type to HTTP Response Mapping

Kotlin `Either<E, T>` maps to WIT `result<T, E>` (`Right` = `Ok`, `Left` = `Err`).

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `Unit` | 204 No Content | empty |
| `T` | 200 OK | JSON-serialized `T` |
| `T?` | 200 OK if non-null, 404 Not Found if `null` | JSON `T` or empty |
| `Either<E, T>` | 200 OK if `Right`, 500 Internal Server Error if `Left` | JSON `T` or JSON `E` |
| `Either<E, Unit>` | 204 No Content if `Right`, 500 if `Left` | empty or JSON `E` |
| `Either<E, T?>` | 200 OK if `Right` non-null, 404 Not Found if `Right(null)`, 500 if `Left` | JSON `T`, empty, or JSON `E` |
| `UnstructuredBinary` | 200 OK (`Url` -> 307 redirect) | Raw binary with Content-Type |
| `UnstructuredText` | 200 OK (`Url` -> 307 redirect) | `text/plain; charset=utf-8` (+ optional `Content-Language`) |

The value inside a non-null `T?` / `Right` follows the same rules as a top-level value: an unstructured value nested in `T?`, `Either<E, _>`, or `Either<E, T?>` is sent as the **raw** body, not as JSON.

## Data Type to JSON Mapping

| Kotlin Type | JSON Representation |
|---|---|
| `String` | JSON string |
| `Int`, `Long` | JSON number (integer) |
| `Float`, `Double` | JSON number (float) |
| `Boolean` | JSON boolean |
| `List<T>` | JSON array |
| `data class` | JSON object (camelCase field names) |
| `T?` | value or `null` |
| `sealed class` / `enum class` | JSON variant |
| `Pair`/`Triple` | JSON array |

## Related Skills

- `golem-add-http-endpoint-kotlin` — mounting, routing and deployment
- `golem-invoke-agent-kotlin` — CLI argument encoding
