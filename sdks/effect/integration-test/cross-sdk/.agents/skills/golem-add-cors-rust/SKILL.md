---
name: golem-add-cors-rust
description: "Configuring CORS for Rust HTTP endpoints. Use when the user asks to enable CORS, allow cross-origin requests, or configure allowed origins for HTTP endpoints."
---

# Configuring CORS for Rust HTTP Endpoints

## Mount-Level CORS

Set `cors` on `#[agent_definition]` to apply allowed origins to **all** endpoints:

```rust
#[agent_definition(
    mount = "/api/{name}",
    cors = ["https://app.example.com"]
)]
pub trait MyAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/data")]
    fn get_data(&self) -> Data;
    // Allows https://app.example.com
}
```

## Endpoint-Level CORS

Set `cors` on `#[endpoint]` to add allowed origins for a specific endpoint. Origins are **unioned** with mount-level CORS:

```rust
#[agent_definition(
    mount = "/api/{name}",
    cors = ["https://app.example.com"]
)]
pub trait MyAgent {
    fn new(name: String) -> Self;

    #[endpoint(get = "/data", cors = ["*"])]
    fn get_data(&self) -> Data;
    // Allows BOTH https://app.example.com AND * (all origins)

    #[endpoint(get = "/other")]
    fn get_other(&self) -> Data;
    // Inherits mount-level: only https://app.example.com
}
```

## Wildcard

Use `"*"` to allow all origins:

```rust
#[agent_definition(mount = "/public/{name}", cors = ["*"])]
pub trait PublicAgent {
    fn new(name: String) -> Self;
}
```

## CORS Preflight

Golem automatically handles `OPTIONS` preflight requests for endpoints that have CORS configured. The preflight response includes `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, and `Access-Control-Allow-Headers` headers.
