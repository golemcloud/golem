---
name: golem-make-http-request-rust
description: "Making outgoing HTTP requests from a Rust Golem agent. Use when the user asks to call an external API, make HTTP requests, use an HTTP client, or send HTTP requests from agent code."
---

# Making Outgoing HTTP Requests (Rust)

## Overview

Golem Rust agents use the `wstd` crate for outgoing HTTP requests. `wstd` provides an async HTTP client built on WASI HTTP — it is included by default in every Golem Rust component's `Cargo.toml`.

> **⚠️ WARNING:** Third-party HTTP client crates like `reqwest`, `ureq`, `hyper` (client), or `surf` will **NOT** work in Golem. These crates depend on native networking (tokio, OpenSSL, etc.) which is not available in WebAssembly. Use `wstd::http` or another crate that targets the WASI HTTP interface.

## Imports

```rust
use wstd::http::{Client, Request, Body, HeaderValue};
```

For JSON support (enabled by default via the `json` feature):
```rust
use serde::{Serialize, Deserialize};
// Body::from_json(&T) for serializing request bodies
// response.body_mut().json::<T>() for deserializing response bodies
```

## GET Request

```rust
let request = Request::get("https://api.example.com/data")
    .header("Accept", HeaderValue::from_static("application/json"))
    .body(Body::empty())?;

let mut response = Client::new().send(request).await?;

let body_bytes = response.body_mut().contents().await?;
let body_str = String::from_utf8_lossy(body_bytes);
```

> **Important:** GET requests require an explicit empty body — use `Body::empty()` or `Body::from(())`.

## GET with JSON Response

```rust
#[derive(Deserialize)]
struct ApiResponse {
    id: u64,
    name: String,
}

let request = Request::get("https://api.example.com/users/1")
    .body(Body::empty())?;

let mut response = Client::new().send(request).await?;
let user: ApiResponse = response.body_mut().json::<ApiResponse>().await?;
```

## POST with JSON Body

```rust
#[derive(Serialize)]
struct CreateUser {
    name: String,
    email: String,
}

let payload = CreateUser {
    name: "Alice".to_string(),
    email: "alice@example.com".to_string(),
};

// Body::from_json serializes the payload to JSON bytes.
// You must also set the Content-Type header manually.
let request = Request::post("https://api.example.com/users")
    .header("Content-Type", "application/json")
    .body(Body::from_json(&payload).expect("Failed to serialize"))?;

let mut response = Client::new().send(request).await?;
```

## POST with Raw Body

```rust
let request = Request::post("https://api.example.com/submit")
    .header("Content-Type", "application/json")
    .body(Body::from(r#"{"key": "value"}"#))?;

let mut response = Client::new().send(request).await?;
```

`Body::from` converts from `&str`, `String`, `Vec<u8>`, `&[u8]`, and `()` (empty body).

## Setting Headers

```rust
let request = Request::get("https://api.example.com/secure")
    .header("Authorization", HeaderValue::from_static("Bearer my-token"))
    .header("Accept", "application/json")          // &str works directly
    .header("X-Custom", HeaderValue::from_str("dynamic-value")?) // fallible for runtime values
    .body(Body::empty())?;
```

## Reading the Response

```rust
let mut response = Client::new().send(request).await?;

// Status
let status = response.status(); // e.g. StatusCode::OK (200)

// Headers
if let Some(ct) = response.headers().get("Content-Type") {
    println!("Content-Type: {}", ct.to_str()?);
}

// Body — choose one:
let bytes = response.body_mut().contents().await?;          // &[u8]
// or
let text = response.body_mut().str_contents().await?;       // &str
// or (with json feature)
let parsed: MyStruct = response.body_mut().json::<MyStruct>().await?;
```

## Error Handling

```rust
let request = Request::get(url).body(Body::empty())?;

match Client::new().send(request).await {
    Ok(mut response) => {
        if response.status().is_success() {
            let data: MyData = response.body_mut().json().await?;
            Ok(data)
        } else {
            let error_body = String::from_utf8_lossy(
                response.body_mut().contents().await?
            ).to_string();
            Err(format!("API error {}: {}", response.status(), error_body))
        }
    }
    Err(e) => Err(format!("Request failed: {}", e)),
}
```

## Timeouts

```rust
use wstd::http::Client;
use std::time::Duration;

let mut client = Client::new();
client.set_connect_timeout(Duration::from_secs(5));
client.set_first_byte_timeout(Duration::from_secs(10));
client.set_between_bytes_timeout(Duration::from_secs(30));

let response = client.send(request).await?;
```

## Complete Example in an Agent

```rust
use golem_rust::{agent_definition, agent_implementation, endpoint, Schema};
use serde::Deserialize;
use wstd::http::{Client, Body, HeaderValue, Request};

#[derive(Clone, Schema, Deserialize)]
pub struct WeatherReport {
    pub temperature: f64,
    pub description: String,
}

#[agent_definition(mount = "/weather/{city}")]
pub trait WeatherAgent {
    fn new(city: String) -> Self;

    #[endpoint(get = "/current")]
    async fn get_current(&self) -> WeatherReport;
}

struct WeatherAgentImpl {
    city: String,
}

#[agent_implementation]
impl WeatherAgent for WeatherAgentImpl {
    fn new(city: String) -> Self {
        Self { city }
    }

    async fn get_current(&self) -> WeatherReport {
        let url = format!(
            "https://api.weather.example.com/current?city={}",
            &self.city
        );

        let request = Request::get(&url)
            .header("Accept", HeaderValue::from_static("application/json"))
            .body(Body::empty())
            .expect("Failed to build request");

        let mut response = Client::new()
            .send(request)
            .await
            .expect("Request failed");

        response
            .body_mut()
            .json::<WeatherReport>()
            .await
            .expect("Failed to parse response")
    }
}
```

## Alternative: `golem-wasi-http`

The `golem-wasi-http` crate provides a **reqwest-inspired API** on top of the same WASI HTTP interface, with additional convenience features. Use it with the `async` and `json` features:

```toml
[dependencies]
golem-wasi-http = { version = "0.2.0", features = ["async", "json"] }
```

```rust
use golem_wasi_http::{Client, Response};

let client = Client::builder()
    .default_headers(my_headers)
    .connect_timeout(Duration::from_secs(5))
    .build()
    .unwrap();

// GET with auth
let response = client
    .get("https://api.example.com/data")
    .bearer_auth("my-token")
    .send()
    .await
    .unwrap();
let data: MyData = response.json().await.unwrap();

// POST with JSON + query params
let response = client
    .post("https://api.example.com/users")
    .json(&payload)
    .query(&[("format", "full")])
    .send()
    .await
    .unwrap();

// Multipart form upload (feature = "multipart")
let form = golem_wasi_http::multipart::Form::new()
    .text("name", "file.txt")
    .file("upload", path)?;
let response = client.post(url).multipart(form).send().await?;
```

**What `golem-wasi-http` adds over `wstd::http`:**
- Reqwest-style builder API — `.get()`, `.post()`, `.bearer_auth()`, `.basic_auth()`, `.query()`, `.form()`
- Multipart form-data uploads (with `multipart` feature)
- Response charset decoding (`.text()` with automatic charset sniffing)
- `.error_for_status()` to convert 4xx/5xx into errors
- `CustomRequestExecution` for manual control over the WASI HTTP request lifecycle — separate steps for sending the body, firing the request, and receiving the response, useful for streaming large request bodies
- Raw stream escape hatch via `response.get_raw_input_stream()` for direct access to the WASI `InputStream`

**When to use `wstd::http` (default, recommended):**
- You are writing new code and want a lightweight, standard async client
- Your requests have simple bodies (JSON, strings, bytes)

**When to use `golem-wasi-http`:**
- You need convenience methods like `.bearer_auth()`, `.query()`, `.form()`, `.multipart()`
- You need streaming request body uploads with manual lifecycle control
- You need response charset decoding or `.error_for_status()`
- You are porting code from a reqwest-based codebase

Both crates use the same underlying WASI HTTP interface and work correctly with Golem's durable execution.

## Calling Golem Agent HTTP Endpoints

When making HTTP requests to other Golem agent endpoints (or your own), the request body must match the **Golem HTTP body mapping convention**: non-binary body parameters are always deserialized from a **JSON object** where each top-level field corresponds to a method parameter name. This is true even when the endpoint has a single body parameter.

For example, given this endpoint definition:

```rust
#[endpoint(post = "/record")]
fn record(&mut self, body: String);
```

The correct HTTP request must send a JSON object with a `body` field — **not** a raw text string:

```rust
// ✅ CORRECT — use Body::from_json with a struct whose fields match parameter names
use serde::Serialize;

#[derive(Serialize)]
struct RecordRequest {
    body: String,
}

let request = Request::post("http://my-app.localhost:9006/recorder/main/record")
    .header("Content-Type", "application/json")
    .body(Body::from_json(&RecordRequest { body: "a".to_string() }).unwrap())?;
Client::new().send(request).await?;

// ✅ ALSO CORRECT — inline JSON via raw body string
let request = Request::post("http://my-app.localhost:9006/recorder/main/record")
    .header("Content-Type", "application/json")
    .body(Body::from(r#"{"body": "a"}"#))?;
Client::new().send(request).await?;

// ❌ WRONG — raw text body does NOT match Golem's JSON body mapping
let request = Request::post("http://my-app.localhost:9006/recorder/main/record")
    .header("Content-Type", "text/plain")
    .body(Body::from("a"))?;
```

> **Rule of thumb:** If the target endpoint is a Golem agent, always send `application/json` with parameter names as JSON keys. Load the `golem-http-params-rust` skill for the full body mapping rules.

## Key Constraints

- Use `wstd::http` (async) or `golem-wasi-http` (reqwest-like; sync by default, async with the `async` feature) — both target the WASI HTTP interface
- **`reqwest`, `ureq`, `hyper` (client), `surf`, and similar crates will NOT work** — they depend on native networking stacks (tokio, OpenSSL) unavailable in WebAssembly
- Third-party crates that internally use one of these clients (e.g., many SDK crates) will also fail to compile or run
- Any crate that targets the WASI HTTP interface (`wasi:http/outgoing-handler`) will work
- When using `wstd::http`: GET requests require an explicit `body(Body::empty())` call
- When using `wstd::http`: use `Body::from_json(&data)` to serialize a struct as a JSON request body, and set the `Content-Type: application/json` header manually
- When using `wstd::http`: use `response.body_mut().json::<T>()` to deserialize a JSON response body
- `wstd` is included by default in Golem Rust project templates; `golem-wasi-http` must be added manually
