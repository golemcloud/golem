use axum::{
    Router,
    routing::{get, post},
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    http::{StatusCode, HeaderValue, HeaderMap},
    Json,
    middleware::{self, Next},
};
use golem_api_grpc::proto::golem::{
    apidefinition::{
        ApiDefinition, ApiDefinitionId, CompiledGatewayBinding, CompiledHttpApiDefinition,
        CompiledHttpRoute, CorsPreflight, Middleware as ApiMiddleware, SecurityWithProviderMetadata,
        StaticBinding,
    },
    rib::{RibInputType, RibOutputType},
    wasm::ast::{Type, type_::Kind},
};
use golem_worker_gateway::{
    openapi::{ApiDefinitionConverter, Error, Result},
    configure_swagger_ui,
};
use reqwest::header;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

// Test data structures
#[derive(Debug, Serialize, Deserialize)]
struct User {
    id: String,
    name: String,
    age: i32,
    tags: Vec<String>,
    metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateUserRequest {
    name: String,
    age: i32,
    tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ErrorResponse {
    code: String,
    message: String,
}

// Mock authentication middleware
async fn auth_middleware(
    headers: HeaderMap,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if let Some(auth) = headers.get("Authorization") {
        if auth == "Bearer test-token" {
            return next.run(request).await;
        }
    }
    
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            code: "UNAUTHORIZED".to_string(),
            message: "Invalid or missing authentication token".to_string(),
        }),
    )
        .into_response()
}

// Mock handlers
async fn get_user(Path(id): Path<String>) -> impl IntoResponse {
    if id == "404" {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: format!("User {} not found", id),
            }),
        )
            .into_response();
    }

    if id == "500" {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                code: "INTERNAL_ERROR".to_string(),
                message: "Internal server error".to_string(),
            }),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(User {
            id,
            name: "Test User".to_string(),
            age: 30,
            tags: vec!["test".to_string()],
            metadata: Some(HashMap::from([("role".to_string(), "user".to_string())])),
        }),
    )
        .into_response()
}

async fn create_user(
    Json(payload): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if payload.age < 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: "INVALID_AGE".to_string(),
                message: "Age cannot be negative".to_string(),
            }),
        )
            .into_response();
    }

    let user = User {
        id: "new-id".to_string(),
        name: payload.name,
        age: payload.age,
        tags: payload.tags,
        metadata: Some(HashMap::new()),
    };

    (StatusCode::CREATED, Json(user)).into_response()
}

#[tokio::test]
async fn test_complex_types_and_errors() -> Result<()> {
    // Create API Definition with complex types
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 0, // GET
                    path: "/api/users/{id}".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: {
                                let mut map = HashMap::new();
                                map.insert("id".to_string(), Type {
                                    kind: Some(Kind::String(Default::default())),
                                });
                                map
                            },
                        }),
                        response_rib_output: Some(RibOutputType {
                            type_: Some(Type {
                                kind: Some(Kind::Record(Default::default())),
                            }),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 1, // POST
                    path: "/api/users".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: {
                                let mut map = HashMap::new();
                                map.insert("user".to_string(), Type {
                                    kind: Some(Kind::Record(Default::default())),
                                });
                                map
                            },
                        }),
                        response_rib_output: Some(RibOutputType {
                            type_: Some(Type {
                                kind: Some(Kind::Record(Default::default())),
                            }),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    // Convert to OpenAPI
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Create test server
    let app = Router::new()
        .route("/api/users/:id", get(get_user))
        .route("/api/users", post(create_user));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Test successful GET with complex response
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let user: User = response.json().await.unwrap();
    assert_eq!(user.id, "123");
    assert!(!user.tags.is_empty());
    assert!(user.metadata.is_some());

    // Test 404 error
    let response = client
        .get(&format!("http://{}/api/users/404", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, "NOT_FOUND");

    // Test 500 error
    let response = client
        .get(&format!("http://{}/api/users/500", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, "INTERNAL_ERROR");

    // Test successful POST with complex request
    let response = client
        .post(&format!("http://{}/api/users", addr))
        .json(&CreateUserRequest {
            name: "New User".to_string(),
            age: 25,
            tags: vec!["new".to_string()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let user: User = response.json().await.unwrap();
    assert_eq!(user.name, "New User");

    // Test validation error
    let response = client
        .post(&format!("http://{}/api/users", addr))
        .json(&CreateUserRequest {
            name: "Invalid User".to_string(),
            age: -1,
            tags: vec![],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let error: ErrorResponse = response.json().await.unwrap();
    assert_eq!(error.code, "INVALID_AGE");

    Ok(())
}

#[tokio::test]
async fn test_security_and_cors() -> Result<()> {
    // Create API Definition with security and CORS
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 0, // GET
                    path: "/api/users/{id}".to_string(),
                    middleware: Some(ApiMiddleware {
                        http_authentication: Some(SecurityWithProviderMetadata {
                            provider: "bearer".to_string(),
                            ..Default::default()
                        }),
                        cors: Some(CorsPreflight {
                            allow_origin: Some("http://localhost:3000".to_string()),
                            allow_methods: vec!["GET".to_string()],
                            allow_headers: vec!["Authorization".to_string()],
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    // Convert to OpenAPI
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Create test server with auth and CORS
    let cors = CorsLayer::new()
        .allow_origin("http://localhost:3000".parse::<HeaderValue>().unwrap())
        .allow_methods([axum::http::Method::GET])
        .allow_headers([header::AUTHORIZATION]);

    let app = Router::new()
        .route("/api/users/:id", get(get_user))
        .layer(cors)
        .layer(middleware::from_fn(auth_middleware));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Test without auth token
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);

    // Test with invalid auth token
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .header("Authorization", "Bearer invalid-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);

    // Test with valid auth token
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // Test CORS preflight
    let response = client
        .request(reqwest::Method::OPTIONS, &format!("http://{}/api/users/123", addr))
        .header("Origin", "http://localhost:3000")
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "authorization")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("access-control-allow-origin").unwrap(),
        "http://localhost:3000"
    );

    // Test CORS with disallowed origin
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .header("Origin", "http://evil.com")
        .header("Authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);

    Ok(())
}

#[tokio::test]
async fn test_static_file_server() -> Result<()> {
    // Create API Definition for static file server
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 0, // GET
                    path: "/static/{path..}".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        binding_type: 1, // StaticBinding
                        static_binding: Some(StaticBinding {
                            root_dir: "static".to_string(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    middleware: Some(ApiMiddleware {
                        cors: Some(CorsPreflight {
                            allow_origin: Some("*".to_string()),
                            allow_methods: vec!["GET".to_string()],
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    // Convert to OpenAPI
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Verify static file serving configuration
    let path = openapi.paths.get("/static/{path}").expect("Path should exist");
    let get = path.get.as_ref().expect("GET operation should exist");
    
    // Check path parameter
    let parameters = get.parameters.as_ref().expect("Parameters should exist");
    assert_eq!(parameters.len(), 1);
    if let Parameter::Path { parameter_data, .. } = &parameters[0] {
        assert_eq!(parameter_data.name, "path");
        if let ParameterSchemaOrContent::Schema(schema) = &parameter_data.format {
            assert!(matches!(schema, Schema::Array { .. }));
        }
    }

    // Check response content types
    let responses = get.responses.as_ref().expect("Responses should exist");
    let ok_response = responses.get("200").expect("200 response should exist");
    let content = ok_response.content.as_ref().expect("Content should exist");
    
    // Verify supported content types
    assert!(content.contains_key("application/json"));
    assert!(content.contains_key("text/html"));
    assert!(content.contains_key("image/*"));
    assert!(content.contains_key("application/octet-stream"));

    // Check CORS headers
    let headers = ok_response.headers.as_ref().expect("Headers should exist");
    assert!(headers.contains_key("Access-Control-Allow-Origin"));

    Ok(())
}

// Rate limiting middleware with a simple token bucket
struct RateLimiter {
    requests: Arc<tokio::sync::Mutex<HashMap<String, (u32, std::time::Instant)>>>,
    max_requests: u32,
    window_secs: u64,
}

impl RateLimiter {
    fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            requests: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            max_requests,
            window_secs,
        }
    }

    async fn is_allowed(&self, client_id: &str) -> bool {
        let mut requests = self.requests.lock().await;
        let now = std::time::Instant::now();
        
        if let Some((count, timestamp)) = requests.get(client_id) {
            if now.duration_since(*timestamp).as_secs() >= self.window_secs {
                requests.insert(client_id.to_string(), (1, now));
                true
            } else if *count < self.max_requests {
                requests.insert(client_id.to_string(), (count + 1, *timestamp));
                true
            } else {
                false
            }
        } else {
            requests.insert(client_id.to_string(), (1, now));
            true
        }
    }
}

async fn rate_limit_middleware(
    State(limiter): State<Arc<RateLimiter>>,
    headers: HeaderMap,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let client_id = headers
        .get("X-Client-ID")
        .map(|h| h.to_str().unwrap_or("default"))
        .unwrap_or("default");

    if limiter.is_allowed(client_id).await {
        next.run(request).await
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                code: "RATE_LIMITED".to_string(),
                message: "Too many requests".to_string(),
            }),
        )
            .into_response()
    }
}

// Logging middleware
async fn logging_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let start = std::time::Instant::now();
    let method = request.method().clone();
    let uri = request.uri().clone();

    let response = next.run(request).await;

    println!(
        "{} {} {} {}ms",
        method,
        uri,
        response.status(),
        start.elapsed().as_millis()
    );

    response
}

#[tokio::test]
async fn test_rate_limiting() -> Result<()> {
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![CompiledHttpRoute {
                method: 0,
                path: "/api/users/{id}".to_string(),
                middleware: Some(ApiMiddleware {
                    rate_limit: Some(Default::default()), // Add rate limit config
                    ..Default::default()
                }),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };

    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Create test server with rate limiting
    let limiter = Arc::new(RateLimiter::new(2, 5)); // 2 requests per 5 seconds
    let app = Router::new()
        .route("/api/users/:id", get(get_user))
        .layer(middleware::from_fn_with_state(
            limiter.clone(),
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn(logging_middleware));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Test rate limiting
    for i in 0..3 {
        let response = client
            .get(&format!("http://{}/api/users/123", addr))
            .header("X-Client-ID", "test-client")
            .send()
            .await
            .unwrap();

        if i < 2 {
            assert_eq!(response.status(), 200);
        } else {
            assert_eq!(response.status(), 429);
            let error: ErrorResponse = response.json().await.unwrap();
            assert_eq!(error.code, "RATE_LIMITED");
        }
    }

    // Wait for rate limit window to reset
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    // Should work again after window reset
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .header("X-Client-ID", "test-client")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    Ok(())
}

#[tokio::test]
async fn test_concurrent_requests() -> Result<()> {
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 0,
                    path: "/api/users/{id}".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 1,
                    path: "/api/users".to_string(),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    let app = Router::new()
        .route("/api/users/:id", get(get_user))
        .route("/api/users", post(create_user))
        .layer(middleware::from_fn(logging_middleware));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Test concurrent GET requests
    let mut handles = Vec::new();
    for id in 1..=10 {
        let client = client.clone();
        let addr = addr.clone();
        handles.push(tokio::spawn(async move {
            let response = client
                .get(&format!("http://{}/api/users/{}", addr, id))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let user: User = response.json().await.unwrap();
            assert_eq!(user.id, id.to_string());
        }));
    }

    // Test concurrent POST requests
    for i in 1..=10 {
        let client = client.clone();
        let addr = addr.clone();
        handles.push(tokio::spawn(async move {
            let response = client
                .post(&format!("http://{}/api/users", addr))
                .json(&CreateUserRequest {
                    name: format!("User {}", i),
                    age: 20 + i,
                    tags: vec![format!("tag{}", i)],
                })
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 201);
        }));
    }

    // Wait for all requests to complete
    for handle in handles {
        handle.await.unwrap();
    }

    Ok(())
}

#[tokio::test]
async fn test_specific_scenarios() -> Result<()> {
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                // Test deeply nested path
                CompiledHttpRoute {
                    method: 0,
                    path: "/api/users/{user_id}/posts/{post_id}/comments/{comment_id}".to_string(),
                    ..Default::default()
                },
                // Test optional query parameters
                CompiledHttpRoute {
                    method: 0,
                    path: "/api/search".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: {
                                let mut map = HashMap::new();
                                map.insert("q".to_string(), Type {
                                    kind: Some(Kind::Option(Box::new(Type {
                                        kind: Some(Kind::String(Default::default())),
                                    }))),
                                });
                                map.insert("page".to_string(), Type {
                                    kind: Some(Kind::Option(Box::new(Type {
                                        kind: Some(Kind::I32(Default::default())),
                                    }))),
                                });
                                map.insert("sort".to_string(), Type {
                                    kind: Some(Kind::Option(Box::new(Type {
                                        kind: Some(Kind::String(Default::default())),
                                    }))),
                                });
                                map
                            },
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                // Test array input
                CompiledHttpRoute {
                    method: 1,
                    path: "/api/batch".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: {
                                let mut map = HashMap::new();
                                map.insert("items".to_string(), Type {
                                    kind: Some(Kind::List(Box::new(Type {
                                        kind: Some(Kind::Record(Default::default())),
                                    }))),
                                });
                                map
                            },
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Verify deeply nested path parameters
    let path = openapi
        .paths
        .get("/api/users/{user_id}/posts/{post_id}/comments/{comment_id}")
        .expect("Path should exist");
    let get = path.get.as_ref().expect("GET operation should exist");
    let parameters = get.parameters.as_ref().expect("Parameters should exist");
    assert_eq!(parameters.len(), 3);

    // Verify optional query parameters
    let path = openapi.paths.get("/api/search").expect("Path should exist");
    let get = path.get.as_ref().expect("GET operation should exist");
    let parameters = get.parameters.as_ref().expect("Parameters should exist");
    for param in parameters {
        if let Parameter::Query { parameter_data, .. } = param {
            assert!(!parameter_data.required);
        }
    }

    // Verify array input
    let path = openapi.paths.get("/api/batch").expect("Path should exist");
    let post = path.post.as_ref().expect("POST operation should exist");
    if let Some(request_body) = &post.request_body {
        let content = request_body.content.get("application/json").expect("JSON content should exist");
        if let Schema::Array { .. } = content.schema.schema_kind {
            // Array schema verified
        } else {
            panic!("Expected array schema");
        }
    }

    Ok(())
}

// Cache middleware using a simple in-memory store
struct Cache {
    store: Arc<tokio::sync::RwLock<HashMap<String, (Vec<u8>, std::time::Instant)>>>,
    ttl_secs: u64,
}

impl Cache {
    fn new(ttl_secs: u64) -> Self {
        Self {
            store: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            ttl_secs,
        }
    }

    async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let store = self.store.read().await;
        if let Some((data, timestamp)) = store.get(key) {
            if timestamp.elapsed().as_secs() < self.ttl_secs {
                return Some(data.clone());
            }
        }
        None
    }

    async fn set(&self, key: String, value: Vec<u8>) {
        let mut store = self.store.write().await;
        store.insert(key, (value, std::time::Instant::now()));
    }
}

// Compression middleware using gzip
async fn compression_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    
    if let Some(body) = response.body_mut().data().await {
        if let Ok(body) = body {
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&body).unwrap();
            let compressed = encoder.finish().unwrap();
            
            *response.body_mut() = axum::body::Body::from(compressed);
            response.headers_mut().insert(
                "Content-Encoding",
                HeaderValue::from_static("gzip"),
            );
        }
    }
    
    response
}

#[tokio::test]
async fn test_caching_and_compression() -> Result<()> {
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![CompiledHttpRoute {
                method: 0,
                path: "/api/users/{id}".to_string(),
                middleware: Some(ApiMiddleware {
                    cache_control: Some(Default::default()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };

    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Create test server with caching and compression
    let cache = Arc::new(Cache::new(60)); // 60 seconds TTL
    let app = Router::new()
        .route("/api/users/:id", get(get_user))
        .layer(middleware::from_fn(compression_middleware))
        .with_state(cache);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Test caching
    let first_response = client
        .get(&format!("http://{}/api/users/123", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(first_response.status(), 200);
    let first_etag = first_response.headers().get("ETag").cloned();

    let cached_response = client
        .get(&format!("http://{}/api/users/123", addr))
        .header("If-None-Match", first_etag.unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(cached_response.status(), 304);

    // Test compression
    let response = client
        .get(&format!("http://{}/api/users/123", addr))
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers().get("Content-Encoding").unwrap(), "gzip");

    Ok(())
}

#[tokio::test]
async fn test_load_scenarios() -> Result<()> {
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 0,
                    path: "/api/users/{id}".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 1,
                    path: "/api/users".to_string(),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    let app = Router::new()
        .route("/api/users/:id", get(get_user))
        .route("/api/users", post(create_user))
        .layer(middleware::from_fn(logging_middleware));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Burst test: 100 requests in quick succession
    let start = std::time::Instant::now();
    let mut handles = Vec::new();
    for i in 0..100 {
        let client = client.clone();
        let addr = addr.clone();
        handles.push(tokio::spawn(async move {
            let response = client
                .get(&format!("http://{}/api/users/{}", addr, i))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    let burst_duration = start.elapsed();
    println!("Burst test completed in {}ms", burst_duration.as_millis());

    // Sustained load test: 10 requests per second for 10 seconds
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let mut handles = Vec::new();
        for i in 0..10 {
            let client = client.clone();
            let addr = addr.clone();
            handles.push(tokio::spawn(async move {
                let response = client
                    .get(&format!("http://{}/api/users/{}", addr, i))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    let sustained_duration = start.elapsed();
    println!("Sustained test completed in {}ms", sustained_duration.as_millis());

    Ok(())
}

#[tokio::test]
async fn test_edge_cases() -> Result<()> {
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                // Test very long path parameter
                CompiledHttpRoute {
                    method: 0,
                    path: "/api/files/{path..}".to_string(),
                    ..Default::default()
                },
                // Test all HTTP methods
                CompiledHttpRoute {
                    method: 0, // GET
                    path: "/api/resource".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 1, // POST
                    path: "/api/resource".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 2, // PUT
                    path: "/api/resource".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 3, // DELETE
                    path: "/api/resource".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 4, // PATCH
                    path: "/api/resource".to_string(),
                    ..Default::default()
                },
                // Test complex query parameters
                CompiledHttpRoute {
                    method: 0,
                    path: "/api/search".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: {
                                let mut map = HashMap::new();
                                // Array parameter
                                map.insert("tags[]".to_string(), Type {
                                    kind: Some(Kind::List(Box::new(Type {
                                        kind: Some(Kind::String(Default::default())),
                                    }))),
                                });
                                // Nested object parameter
                                map.insert("filter".to_string(), Type {
                                    kind: Some(Kind::Record(Default::default())),
                                });
                                // Date parameter
                                map.insert("date".to_string(), Type {
                                    kind: Some(Kind::String(Default::default())),
                                });
                                map
                            },
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;

    // Verify path parameter handling
    let path = openapi.paths.get("/api/files/{path}").expect("Path should exist");
    let get = path.get.as_ref().expect("GET operation should exist");
    let parameters = get.parameters.as_ref().expect("Parameters should exist");
    assert_eq!(parameters.len(), 1);

    // Verify all HTTP methods
    let path = openapi.paths.get("/api/resource").expect("Path should exist");
    assert!(path.get.is_some());
    assert!(path.post.is_some());
    assert!(path.put.is_some());
    assert!(path.delete.is_some());
    assert!(path.patch.is_some());

    // Verify complex query parameters
    let path = openapi.paths.get("/api/search").expect("Path should exist");
    let get = path.get.as_ref().expect("GET operation should exist");
    let parameters = get.parameters.as_ref().expect("Parameters should exist");
    for param in parameters {
        if let Parameter::Query { parameter_data, .. } = param {
            match parameter_data.name.as_str() {
                "tags[]" => {
                    if let ParameterSchemaOrContent::Schema(Schema::Array { .. }) = &parameter_data.format {
                        // Array parameter verified
                    } else {
                        panic!("Expected array parameter");
                    }
                }
                "filter" => {
                    if let ParameterSchemaOrContent::Schema(Schema::Object { .. }) = &parameter_data.format {
                        // Object parameter verified
                    } else {
                        panic!("Expected object parameter");
                    }
                }
                "date" => {
                    if let ParameterSchemaOrContent::Schema(Schema::String { format: Some(format), .. }) = &parameter_data.format {
                        assert_eq!(format, "date-time");
                    } else {
                        panic!("Expected date parameter");
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}
