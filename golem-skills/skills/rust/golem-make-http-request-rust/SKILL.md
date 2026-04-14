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
use wstd::http::{Client, Request, HeaderValue};
use wstd::io::{empty, AsyncRead};
```

For JSON support (enabled by default via the `json` feature):
```rust
use wstd::http::request::JsonRequest; // for .json() on request builder
use serde::{Serialize, Deserialize};
```

## GET Request

```rust
let request = Request::get("https://api.example.com/data")
    .header("Accept", HeaderValue::from_static("application/json"))
    .body(empty())?;

let mut response = Client::new().send(request).await?;

let body_bytes = response.body_mut().bytes().await?;
let body_str = String::from_utf8_lossy(&body_bytes);
```

> **Important:** GET requests require an explicit empty body — use `wstd::io::empty()`.

## GET with JSON Response

```rust
use wstd::http::request::JsonRequest;

#[derive(Deserialize)]
struct ApiResponse {
    id: u64,
    name: String,
}

let request = Request::get("https://api.example.com/users/1")
    .body(empty())?;

let mut response = Client::new().send(request).await?;
let user: ApiResponse = response.body_mut().json::<ApiResponse>().await?;
```

## POST with JSON Body

```rust
use wstd::http::request::JsonRequest;

#[derive(Serialize)]
struct CreateUser {
    name: String,
    email: String,
}

let payload = CreateUser {
    name: "Alice".to_string(),
    email: "alice@example.com".to_string(),
};

// .json() serializes the payload and sets Content-Type automatically
let request = Request::post("https://api.example.com/users")
    .json(&payload)?;

let mut response = Client::new().send(request).await?;
```

## POST with Raw Body

```rust
use wstd::http::IntoBody;

let request = Request::post("https://api.example.com/submit")
    .header("Content-Type", "application/json")
    .body(r#"{"key": "value"}"#.into_body())?;

let mut response = Client::new().send(request).await?;
```

`IntoBody` converts from `&str`, `String`, `Vec<u8>`, and `&[u8]`.

## Setting Headers

```rust
let request = Request::get("https://api.example.com/secure")
    .header("Authorization", HeaderValue::from_static("Bearer my-token"))
    .header("Accept", "application/json")          // &str works directly
    .header("X-Custom", HeaderValue::from_str("dynamic-value")?) // fallible for runtime values
    .body(empty())?;
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
let bytes: Vec<u8> = response.body_mut().bytes().await?;
// or
let text = String::from_utf8(response.body_mut().bytes().await?)?;
// or (with json feature)
let parsed: MyStruct = response.body_mut().json::<MyStruct>().await?;
```

## Error Handling

```rust
let request = Request::get(url).body(empty())?;

match Client::new().send(request).await {
    Ok(mut response) => {
        if response.status().is_success() {
            let data: MyData = response.body_mut().json().await?;
            Ok(data)
        } else {
            let error_body = String::from_utf8_lossy(
                &response.body_mut().bytes().await?
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
use wstd::http::{Client, HeaderValue, Request};
use wstd::http::request::JsonRequest;
use wstd::io::empty;

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
            .body(empty())
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

## Key Constraints

- Use `wstd::http` (async) or `golem-wasi-http` (sync, reqwest-like) — both target the WASI HTTP interface
- **`reqwest`, `ureq`, `hyper` (client), `surf`, and similar crates will NOT work** — they depend on native networking stacks (tokio, OpenSSL) unavailable in WebAssembly
- Third-party crates that internally use one of these clients (e.g., many SDK crates) will also fail to compile or run
- Any crate that targets the WASI HTTP interface (`wasi:http/outgoing-handler`) will work
- When using `wstd::http`: GET requests require an explicit `body(empty())` call
- When using `wstd::http`: import `wstd::http::request::JsonRequest` to use the `.json()` builder method
- `wstd` is included by default in Golem Rust project templates; `golem-wasi-http` must be added manually
