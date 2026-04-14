---
name: golem-http-params-rust
description: "Mapping HTTP request elements to Rust agent parameters. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response type mapping for HTTP endpoints."
---

# HTTP Request and Response Parameter Mapping (Rust)

## Overview

When an agent is exposed over HTTP, Golem maps parts of each HTTP request to constructor and method parameters. This skill covers how path segments, query parameters, headers, and request bodies are mapped, which types are supported for each, and how return types map to HTTP responses.

## Path Variables

Path variables `{var_name}` in mount or endpoint paths map to parameters by name:

```rust
// Mount path variables → constructor parameters
#[agent_definition(mount = "/api/tasks/{task_name}")]
pub trait TaskAgent {
    fn new(task_name: String) -> Self;

    // Endpoint path variables → method parameters
    #[endpoint(get = "/items/{item_id}")]
    fn get_item(&self, item_id: String) -> Item;
}
```

Remaining (catch-all) path variables capture everything after a prefix:

```rust
#[endpoint(get = "/files/{*path}")]
fn get_file(&self, path: String) -> FileContent;
// GET .../files/docs/readme.md → path = "docs/readme.md"
```

Catch-all variables can only appear as the **last** path segment and are **not** allowed in mount paths.

## Query Parameters

Specified in the endpoint path using `?key={var}` syntax:

```rust
#[endpoint(get = "/search?q={query}&limit={max_results}")]
fn search(&self, query: String, max_results: u64) -> Vec<SearchResult>;
// GET .../search?q=hello&limit=10
```

## Header Variables

Map HTTP headers to parameters using the `headers(...)` block on `#[endpoint]`:

```rust
#[endpoint(
    get = "/data",
    headers("X-Request-Id" = "request_id", "Authorization" = "token")
)]
fn get_data(&self, request_id: String, token: String) -> Data;
```

## Supported Types for Path, Query, and Header Variables

Only these types can be used for parameters bound to path/query/header variables (the value is parsed from the URL/header string):

| Rust Type | Parsed From |
|---|---|
| `String` | Used as-is |
| `char` | Single character |
| `bool` | Parsed from `"true"` / `"false"` |
| `u8`, `u16`, `u32`, `u64` | Parsed as unsigned integer |
| `i8`, `i16`, `i32`, `i64` | Parsed as signed integer |
| `f32`, `f64` | Parsed as floating-point number |
| Enum (unit variants only) | Matched against known case names |

**For query parameters and headers only** (not path variables), two additional wrapper types are supported:

| Rust Type | Behavior |
|---|---|
| `Option<T>` (where `T` is a supported type above) | Optional — absent query param or header produces `None` |
| `Vec<T>` (where `T` is a supported type above) | Repeated query params or comma-separated header values |

**All other types** (structs, tuples, enums with data, `HashMap`, etc.) can only be used as **body parameters**.

## POST Request Body Mapping

For `POST`/`PUT`/`DELETE` endpoints, method parameters **not** bound to path variables, query parameters, or headers are populated from the JSON request body:

```rust
#[endpoint(post = "/items/{id}")]
fn update_item(&mut self, id: String, name: String, count: u64) -> Item;
// POST .../items/123
// Body: { "name": "Widget", "count": 5 }
// → id from path, name and count from body
```

Each unmapped parameter becomes a top-level field in the expected JSON body object. All custom types must derive `Schema`.

## Binary Request and Response Bodies

Use `UnstructuredBinary` from the SDK for raw binary payloads:

```rust
use golem_rust::agentic::UnstructuredBinary;
use golem_rust::AllowedMimeTypes;

// Accepting any binary content type
#[endpoint(post = "/upload/{bucket}")]
fn upload(&self, bucket: String, payload: UnstructuredBinary<String>) -> i64;

// Restricting to specific MIME types
#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum ImageTypes {
    #[mime_type("image/gif")]
    ImageGif,
    #[mime_type("image/png")]
    ImagePng,
}

#[endpoint(post = "/upload-image/{bucket}")]
fn upload_image(&self, bucket: String, payload: UnstructuredBinary<ImageTypes>) -> i64;

// Returning binary data
#[endpoint(get = "/download")]
fn download(&self) -> UnstructuredBinary<String>;
```

In the implementation:
```rust
fn upload(&self, _bucket: String, payload: UnstructuredBinary<String>) -> i64 {
    match payload {
        UnstructuredBinary::Url(_) => -1,
        UnstructuredBinary::Inline { data, .. } => data.len() as i64,
    }
}

fn download(&self) -> UnstructuredBinary<String> {
    UnstructuredBinary::Inline {
        data: vec![1, 2, 3, 4],
        mime_type: "application/octet-stream".to_string(),
    }
}
```

## Return Type to HTTP Response Mapping

| Return Type | HTTP Status | Response Body |
|---|---|---|
| `()` (unit / no return) | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |
| `Option<T>` | 200 OK if `Some`, 404 Not Found if `None` | JSON `T` or empty |
| `Result<T, E>` | 200 OK if `Ok`, 500 Internal Server Error if `Err` | JSON `T` or JSON `E` |
| `Result<(), E>` | 204 No Content if `Ok`, 500 if `Err` | empty or JSON `E` |
| `Result<T, ()>` | 200 OK if `Ok`, 500 if `Err` | JSON `T` or empty |
| `UnstructuredBinary<M>` | 200 OK | Raw binary with Content-Type |

## Data Type to JSON Mapping

| Rust Type | JSON Representation |
|---|---|
| `String` | JSON string |
| `u8`–`u64`, `i8`–`i64` | JSON number (integer) |
| `f32`, `f64` | JSON number (float) |
| `bool` | JSON boolean |
| `Vec<T>` | JSON array |
| Struct (with `Schema`) | JSON object (camelCase field names) |
| `Option<T>` | value or `null` |
| `Result<T, E>` | value (see response mapping above) |
| Enum (unit variants) | JSON string |
| Enum (with data) | JSON object with tag |
| `HashMap<K, V>` | JSON array of `[key, value]` tuples |
