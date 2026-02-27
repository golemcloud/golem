---
name: modifying-http-endpoints
description: "Adding or modifying HTTP REST API endpoints in Golem services. Use when creating new endpoints, changing existing API routes, or updating request/response types for the Golem REST API."
---

# Modifying HTTP Endpoints

## Framework

Golem uses **Poem** with **poem-openapi** for REST API endpoints. Endpoints are defined as methods on API structs annotated with `#[OpenApi]` and `#[oai]`.

## Where Endpoints Live

- **Worker service**: `golem-worker-service/src/api/` — worker lifecycle, invocation, oplog
- **Registry service**: `golem-registry-service/src/api/` — components, environments, deployments, plugins, accounts

Each service has an `api/mod.rs` that defines an `Apis` type tuple and a `make_open_api_service` function combining all API structs.

## Adding a New Endpoint

### 1. Define the endpoint method

Add a method to the appropriate API struct (e.g., `WorkerApi`, `ComponentsApi`):

```rust
#[oai(
    path = "/:component_id/workers/:worker_name/my-action",
    method = "post",
    operation_id = "my_action"
)]
async fn my_action(
    &self,
    component_id: Path<ComponentId>,
    worker_name: Path<String>,
    request: Json<MyRequest>,
    token: GolemSecurityScheme,
) -> Result<Json<MyResponse>> {
    // ...
}
```

### 2. If adding a new API struct

1. Create a new file in the service's `api/` directory
2. Define a struct and impl block with `#[OpenApi(prefix_path = "/v1/...", tag = ApiTags::...)]`
3. Add it to the `Apis` type tuple in `api/mod.rs`
4. Instantiate it in `make_open_api_service`

### 3. Request/response types

- Define types in `golem-common/src/model/` with `poem_openapi::Object` derive
- If the type is used in the generated client, add it to the type mapping in `golem-client/build.rs`

## After Modifying Endpoints

After any endpoint change, you **must** regenerate and rebuild:

### Step 1: Regenerate OpenAPI specs

```shell
cargo make generate-openapi
```

This builds the services, dumps their OpenAPI YAML, merges them, and stores the result in `openapi/`.

### Step 2: Clean and rebuild golem-client

The `golem-client` crate auto-generates its code from the OpenAPI spec at build time via `build.rs`. After regenerating the specs:

```shell
cargo clean -p golem-client
cargo build -p golem-client
```

The clean step is necessary because the build script uses `rerun-if-changed` on the YAML file, but cargo may cache stale generated code.

### Step 3: If new types are used in the client

Add type mappings in `golem-client/build.rs` to the `gen()` call's type replacement list. This maps OpenAPI schema names to existing Rust types from `golem-common` or `golem-wasm`.

### Step 4: Build and verify

```shell
cargo make build
```

Then run the appropriate tests:

- HTTP API tests: `cargo make api-tests-http`
- gRPC API tests: `cargo make api-tests-grpc`

## Checklist

1. Endpoint method added with `#[oai]` annotation
2. New API struct registered in `api/mod.rs` `Apis` tuple and `make_open_api_service` (if applicable)
3. Request/response types defined in `golem-common` with `poem_openapi::Object`
4. Type mappings added in `golem-client/build.rs` (if applicable)
5. `cargo make generate-openapi` run
6. `cargo clean -p golem-client && cargo build -p golem-client` run
7. `cargo make build` succeeds
8. `cargo make fix` run before PR
