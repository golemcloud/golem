# Golem Worker Service Base

This crate provides the base functionality for Golem worker services, including API definition management, gateway bindings, and OpenAPI integration.

## OpenAPI Export Feature

The OpenAPI export feature allows you to convert Golem API Definitions into OpenAPI 3.0 specifications,
making it easy to document and consume your APIs using standard tools and client libraries.

### Features

- Full and lossless conversion of API Definitions to OpenAPI 3.0
- Support for both JSON and YAML output formats
- Comprehensive type mapping from RIB to OpenAPI schemas
- Security scheme definitions (Bearer, Basic, API Key, OAuth2)
- CORS configuration via OpenAPI extensions
- Swagger UI integration

### Usage

1. **Basic Export**

```rust
use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition;
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};

// Create an API definition
let api_def = HttpApiDefinition::new(
    ApiDefinitionId("my-api".to_string()),
    ApiVersion("1.0".to_string()),
);

// Export to OpenAPI JSON
let json_spec = api_def.to_openapi_string("json").unwrap();

// Export to OpenAPI YAML
let yaml_spec = api_def.to_openapi_string("yaml").unwrap();
```

2. **Adding Security**

```rust
use golem_api_grpc::proto::golem::apidefinition::AuthCallBack;

// Add JWT Bearer authentication
let auth = AuthCallBack {
    auth_type: "bearer".to_string(),
    provider_url: "https://auth.example.com".to_string(),
    scopes: vec!["read".to_string(), "write".to_string()],
};

if let GatewayBinding::Http(ref mut binding) = route.binding {
    binding.security = Some(auth);
}
```

3. **Adding CORS**

```rust
use golem_api_grpc::proto::golem::apidefinition::CorsPreflight;

// Add CORS configuration
let cors = CorsPreflight {
    allowed_origins: Some(vec!["https://example.com".to_string()]),
    allowed_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
    allowed_headers: Some(vec!["Content-Type".to_string()]),
    max_age: Some(3600),
    allow_credentials: Some(true),
    expose_headers: Some(vec!["X-Custom-Header".to_string()]),
};

if let GatewayBinding::Http(ref mut binding) = route.binding {
    binding.cors = Some(cors);
}
```

4. **Enabling Swagger UI**

```rust
use golem_worker_service_base::gateway_api_definition::http::SwaggerUiConfig;

// Configure Swagger UI
let config = SwaggerUiConfig {
    enabled: true,
    path: "/docs".to_string(),
    title: Some("My API Documentation".to_string()),
    theme: Some("dark".to_string()),
    ..Default::default()
};

let api_def = HttpApiDefinition::new(
    ApiDefinitionId("my-api".to_string()),
    ApiVersion("1.0".to_string()),
).with_swagger_ui(config);
```

### Type Mapping

The following table shows how RIB types are mapped to OpenAPI schema types:

| RIB Type    | OpenAPI Type |
|-------------|--------------|
| Bool        | boolean      |
| U8-U64      | integer      |
| S8-S64      | integer      |
| F32         | number       |
| F64         | number       |
| Str         | string       |
| List        | array        |
| Option      | nullable     |
| Result      | oneOf        |
| Record      | object       |
| Enum        | enum         |
| Tuple       | array        |

### Client Generation

The OpenAPI specification can be used to generate client libraries in various languages using the OpenAPI Generator:

```bash
# Generate TypeScript client
openapi-generator-cli generate -i openapi.json -g typescript-fetch -o typescript-client

# Generate Python client
openapi-generator-cli generate -i openapi.json -g python -o python-client

# Generate Rust client
openapi-generator-cli generate -i openapi.json -g rust -o rust-client
```

### Testing

The crate includes comprehensive tests for the OpenAPI export functionality:

- Unit tests for type mapping and schema generation
- Integration tests for complex API scenarios
- System tests for client library generation
- Tests for security and CORS configurations

Run the tests using:

```bash
cargo test
```

### Contributing

Contributions are welcome! Please feel free to submit a Pull Request. 