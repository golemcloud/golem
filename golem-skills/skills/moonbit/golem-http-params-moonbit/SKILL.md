---
name: golem-http-params-moonbit
description: "Mapping path, query, header, and body parameters for HTTP endpoints in MoonBit Golem agents. Use when configuring how HTTP request data maps to agent method parameters."
---

# HTTP Request and Response Parameter Mapping (MoonBit)

## Overview

When an agent is exposed over HTTP, Golem maps parts of each HTTP request to constructor and method parameters. This skill covers how path segments, query parameters, headers, and request bodies are mapped, which types are supported for each, and how return types map to HTTP responses.

## Path Variables

Path variables `{var_name}` in mount or endpoint paths map to parameters by name:

```moonbit
///|
/// A task management agent
#derive.agent
#derive.mount("/api/tasks/{task_name}")
pub(all) struct TaskAgent {
  task_name : String
}

///|
fn TaskAgent::new(task_name : String) -> TaskAgent {
  { task_name }
}

///|
/// Get an item by ID
#derive.endpoint(get="/items/{item_id}")
pub fn TaskAgent::get_item(self : Self, item_id : String) -> Item {
  // task_name from mount path, item_id from endpoint path
  ...
}
```

Remaining (catch-all) path variables capture everything after a prefix:

```moonbit
#derive.endpoint(get="/files/{*path}")
pub fn TaskAgent::get_file(self : Self, path : String) -> FileContent {
  // GET .../files/docs/readme.md → path = "docs/readme.md"
  ...
}
```

Catch-all variables can only appear as the **last** path segment and are **not** allowed in mount paths.

## Query Parameters

Specified in the endpoint path using `?key={var}` syntax:

```moonbit
#derive.endpoint(get="/search?q={query}&limit={max_results}")
pub fn TaskAgent::search(self : Self, query : String, max_results : UInt64) -> Array[SearchResult] {
  // GET .../search?q=hello&limit=10
  ...
}
```

## Header Variables

Map HTTP headers to parameters using `#derive.endpoint_header`:

```moonbit
#derive.endpoint(get="/data")
#derive.endpoint_header("X-Request-Id", "request_id")
#derive.endpoint_header("Authorization", "token")
pub fn TaskAgent::get_data(self : Self, request_id : String, token : String) -> Data {
  ...
}
```

Each `#derive.endpoint_header` maps one header to one method parameter. Duplicate header names (case-insensitive) are not allowed.

## Supported Types for Path, Query, and Header Variables

Only these types can be used for parameters bound to path/query/header variables (the value is parsed from the URL/header string):

| MoonBit Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `Char` | Single character |
| `Bool` | Parsed from `"true"` / `"false"` |
| `UInt`, `UInt64` | Parsed as unsigned integer |
| `Int`, `Int64` | Parsed as signed integer |
| `Double` | Parsed as floating-point number |
| Enum (unit variants only) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper types are supported:

| MoonBit Type | Behavior |
|---|---|
| `T?` (`Option[T]`, where `T` is a supported type above) | Optional — absent query param or header produces `None` |
| `Array[T]` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (structs, tuples, enums with data, etc.) can only be used as **body parameters**.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters **not** bound to path variables, query parameters, or headers are populated from the JSON request body:

```moonbit
#derive.endpoint(post="/items/{id}")
pub fn TaskAgent::update_item(self : Self, id : String, name : String, count : UInt64) -> Item {
  // POST .../items/123
  // Body: { "name": "Widget", "count": 5 }
  // → id from path, name and count from body
  ...
}
```

Each unmapped parameter becomes a top-level field in the expected JSON body object. All custom types must have `#derive.golem_schema`.

> **⚠️ Important for callers:** When making HTTP requests *to* a Golem agent endpoint, always send a JSON object with the parameter names as keys — even for a single `String` body parameter. For example, `{"name": "Widget", "count": 5}`, **not** a raw text string.

## Binary Request and Response Bodies

Use `UnstructuredBinary` from the SDK for raw binary payloads:

```moonbit
///|
/// Accepting any binary content type
#derive.endpoint(post="/upload/{bucket}")
pub fn TaskAgent::upload(self : Self, bucket : String, payload : UnstructuredBinary) -> Int64 {
  match payload {
    Url(_) => -1L
    Inline(data~, mime_type~) => data.length().to_int64()
  }
}

///|
/// Restricting to specific MIME types
#derive.endpoint(post="/upload-image/{bucket}")
#derive.mime_types("payload", "image/png", "image/jpeg")
pub fn TaskAgent::upload_image(self : Self, bucket : String, payload : UnstructuredBinary) -> Int64 {
  match payload {
    Url(_) => -1L
    Inline(data~, mime_type~) => data.length().to_int64()
  }
}

///|
/// Returning binary data
#derive.endpoint(get="/download")
pub fn TaskAgent::download(self : Self) -> UnstructuredBinary {
  UnstructuredBinary::from_inline(b"\x01\x02\x03\x04", mime_type="application/octet-stream")
}
```

`UnstructuredBinary` parameters cannot be bound to path, query, or header variables.

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `Unit` (no return) | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |
| `T?` (`Option[T]`) | 200 OK if `Some`, 404 Not Found if `None` | JSON `T` or empty |
| `Result[T, E]` | 200 OK if `Ok`, 500 Internal Server Error if `Err` | JSON `T` or JSON `E` |
| `Result[Unit, E]` | 204 No Content if `Ok`, 500 if `Err` | empty or JSON `E` |
| `UnstructuredBinary` | 200 OK | Raw binary with Content-Type |

## Data Type to JSON Mapping

| MoonBit Type | JSON Representation |
|---|---|
| `String` | JSON string |
| `Int`, `Int64`, `UInt`, `UInt64` | JSON number (integer) |
| `Double` | JSON number (float) |
| `Bool` | JSON boolean |
| `Array[T]` | JSON array |
| Struct (with `#derive.golem_schema`) | JSON object |
| `T?` (`Option[T]`) | value or `null` |
| `Result[T, E]` | value (see response mapping above) |
| Enum (unit variants) | JSON string |
| Enum (with data, `#derive.golem_schema`) | JSON object with tag |
| `(A, B, ...)` (tuple) | JSON array |

## Custom Types

All custom structs and enums used as parameters or return types must be annotated with `#derive.golem_schema`:

```moonbit
#derive.golem_schema
pub(all) enum Priority {
  Low
  Medium
  High
} derive(Eq)

#derive.golem_schema
pub(all) struct TaskInfo {
  title : String
  priority : Priority
  description : String?
}
```

## Complete Example

```moonbit
///|
/// A REST API agent exposing weather data via HTTP endpoints
#derive.agent
#derive.mount("/api/{city}/weather")
pub(all) struct WeatherAgent {
  city : String
  mut last_temperature : Double
}

///|
fn WeatherAgent::new(city : String) -> WeatherAgent {
  { city, last_temperature: 0.0 }
}

///|
/// Get current temperature
#derive.endpoint(get="/current?unit={unit}")
pub fn WeatherAgent::get_temperature(self : Self, unit : String) -> Double {
  if unit == "fahrenheit" {
    self.last_temperature * 9.0 / 5.0 + 32.0
  } else {
    self.last_temperature
  }
}

///|
/// Set the temperature
#derive.endpoint(post="/set")
#derive.endpoint_header("X-Source", "source")
pub fn WeatherAgent::set_temperature(
  self : Self,
  temperature : Double,
  source : String,
) -> String {
  self.last_temperature = temperature
  "Temperature set to " + temperature.to_string() + " from " + source
}
```
