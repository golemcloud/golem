---
name: golem-make-http-request-rust
description: "Makes outgoing HTTP requests from a Rust Golem agent using WASI HTTP p3. Use when calling an external API, sending HTTP requests, or reading HTTP response bodies from Rust agent code."
---

# Making Outgoing HTTP Requests (Rust)

Use `golem_rust::http` for outgoing WASI HTTP p3 requests. It reexports standard `http::Request` and `http::Response` types. Do not use clients that require native sockets, such as the default transports in `reqwest`, `ureq`, or `hyper`.

## GET and response bodies

Run these snippets inside an async agent method. The `?` operator requires a return type that can represent request builder, SDK, and serialization errors; map them into your agent's error type as needed.

```rust
use golem_rust::http::{self, Request};

let request = Request::get("https://api.example.com/data")
    .header("Accept", "application/json")
    .body(Vec::new())?;
let response = http::send(request).await?;

// Inspect status and headers before consuming the response.
let status = response.status();
let content_type = response.headers().get("Content-Type").cloned();
let text = http::response_text(response).await?;
if !status.is_success() {
    // Map the status and, if appropriate, the error body into your agent's error type.
}
```

`send` accepts `Request<Vec<u8>>` and returns `Result<Response, Error>`. Always supply a body, including `Vec::new()` for an empty GET body. The default `Response` body is `golem_rust::wasip3::http_compat::IncomingResponseBody`.

Choose one consuming helper:

- `http::response_bytes(response).await?` returns `bytes::Bytes`.
- `http::response_text(response).await?` returns `String`.
- `http::response_json::<T>(response).await?` returns a deserialized `T: serde::de::DeserializeOwned`. It requires the SDK's `json` feature, enabled by default.

These helpers collect the whole response body in memory. A response can be consumed only once; copy any status or headers you need first.

## JSON POST and authentication

Serialize the request with `serde_json::to_vec` and explicitly set `Content-Type`. Authentication uses ordinary headers, not a client-specific authentication builder. Obtain credentials from agent configuration rather than embedding them in source, and never log them.

```rust
use golem_rust::http::{self, Request};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct CreateUser {
    name: String,
}

#[derive(Deserialize)]
struct User {
    id: u64,
    name: String,
}

async fn create_user(
    token: &str,
) -> Result<User, Box<dyn std::error::Error>> {
    let payload = CreateUser { name: "Alice".to_string() };
    let request = Request::post("https://api.example.com/users")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(serde_json::to_vec(&payload)?)?;

    let response = http::send(request).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("API returned HTTP {status}").into());
    }
    Ok(http::response_json(response).await?)
}
```

Use `create_user` as an internal helper and map its error into the agent's public error type. For raw request bodies, supply a `Vec<u8>` and the appropriate content type. Encode URL query parameters explicitly; there is no `.query()` convenience method.

## Errors and status codes

- Request construction and JSON serialization can fail before sending.
- `send` errors report request/transport failures. An HTTP `404` or `500` is still an `Ok(Response)`; inspect `response.status()` yourself.
- Reading or decoding the body can fail after a response arrives. Handle these failures separately from an unsuccessful HTTP status.
- The convenience API does not automatically reject status codes, retry requests, or follow redirects. Decide explicitly how to handle `3xx`, `4xx`, and `5xx` responses. Do not blindly retry side-effecting requests.

## Streaming and low-level p3

For large responses, consume `response.into_body()` incrementally with `http_body_util::BodyExt` rather than collecting it with a response helper. Add `http-body-util` to the component dependencies if needed. Handle data frames, trailers, and stream errors, and finish consuming the body when the operation requires its full result.

The convenience `send` buffers the request body as `Vec<u8>`. For streaming uploads or direct protocol control, use `golem_rust::wasip3::http::client` and `golem_rust::wasip3::http::types`. The SDK also reexports `golem_rust::wasip3::http_compat` for adapting standard HTTP types to p3. Use the p3 APIs, not Preview 2 outgoing-handler or input-stream examples. Native sockets are not available.

## Calling Golem agent HTTP endpoints

For structured endpoint bodies, send a JSON object whose top-level keys are body parameter names. Include only parameters not bound to path, query, or header values. For example, an endpoint parameter `body: String` needs `{"body":"a"}`, not a bare string. A single object parameter named `payload` needs `{"payload":{"name":"Widget","count":5}}`, not the bare inner object.

```rust
let request = Request::post("http://my-app.localhost:9006/recorder/main/record")
    .header("Content-Type", "application/json")
    .body(serde_json::to_vec(&serde_json::json!({ "body": "a" }))?)?;
let response = http::send(request).await?;
```

Unstructured text and binary parameters instead consume the raw body without this wrapper. Load `golem-http-params-rust` for the restrictions and the separate CLI positional/native-literal rules; do not reuse the HTTP wrapper for CLI arguments.
