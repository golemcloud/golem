use axum::{
    Router,
    routing::{get, post},
    extract::State,
    response::IntoResponse,
    http::{StatusCode, HeaderValue},
    Json,
};
use golem_api_grpc::proto::golem::{
    apidefinition::{
        ApiDefinition, ApiDefinitionId, CompiledGatewayBinding, CompiledHttpApiDefinition,
        CompiledHttpRoute, CorsPreflight, GatewayBindingType, Middleware, SecurityWithProviderMetadata,
        StaticBinding,
    },
    rib::{RibInputType, RibOutputType},
    wasm::ast::{Type, type_::Kind},
};
use golem_worker_gateway::{
    openapi::{ApiDefinitionConverter, SwaggerUiHandler},
    configure_swagger_ui,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;
use tower_http::cors::CorsLayer;

// Test data structures
#[derive(Debug, Serialize, Deserialize)]
struct User {
    id: String,
    name: String,
    age: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateUserRequest {
    name: String,
    age: i32,
}

// Mock worker handler
async fn create_user(Json(payload): Json<CreateUserRequest>) -> impl IntoResponse {
    let user = User {
        id: "123".to_string(),
        name: payload.name,
        age: payload.age,
    };
    (StatusCode::CREATED, Json(user))
}

#[tokio::test]
async fn test_complete_api_flow() {
    // 1. Create an API Definition
    let api = create_test_api_definition();

    // 2. Convert to OpenAPI
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api).unwrap();
    let spec_json = serde_json::to_string(&openapi).unwrap();

    // 3. Create a test server with both API and Swagger UI
    let mut app = Router::new();

    // Add the actual API endpoint
    app = app.route("/api/users", post(create_user));

    // Add Swagger UI
    configure_swagger_ui(&mut app, &api, "/docs", Some("*".to_string()))
        .await
        .unwrap();

    // Add CORS
    let cors = CorsLayer::new()
        .allow_origin("*".parse::<HeaderValue>().unwrap())
        .allow_methods(vec!["GET", "POST"])
        .allow_headers(vec!["content-type"]);
    
    let app = app.layer(cors);

    // 4. Start the test server
    let server = axum::Server::bind(&"127.0.0.1:0".parse().unwrap())
        .serve(app.into_make_service());
    let addr = server.local_addr();
    tokio::spawn(server);

    // 5. Test the Swagger UI endpoint
    let client = reqwest::Client::new();
    let response = client
        .get(&format!("http://{}/docs", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // 6. Test the OpenAPI spec endpoint
    let response = client
        .get(&format!("http://{}/docs/spec", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let spec_text = response.text().await.unwrap();
    assert!(spec_text.contains("openapi"));

    // 7. Test the actual API endpoint
    let response = client
        .post(&format!("http://{}/api/users", addr))
        .json(&CreateUserRequest {
            name: "Test User".to_string(),
            age: 30,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let user: User = response.json().await.unwrap();
    assert_eq!(user.name, "Test User");
    assert_eq!(user.age, 30);
}

fn create_test_api_definition() -> ApiDefinition {
    ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 1, // POST
                    path: "/api/users".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: {
                                let mut map = HashMap::new();
                                // Define CreateUserRequest structure
                                map.insert("name".to_string(), Type {
                                    kind: Some(Kind::String(Default::default())),
                                });
                                map.insert("age".to_string(), Type {
                                    kind: Some(Kind::I32(Default::default())),
                                });
                                map
                            },
                        }),
                        response_rib_output: Some(RibOutputType {
                            type_: Some(Type {
                                kind: Some(Kind::Record(Default::default())), // User record
                            }),
                        }),
                        ..Default::default()
                    }),
                    middleware: Some(Middleware {
                        cors: Some(CorsPreflight {
                            allow_origin: Some("*".to_string()),
                            allow_methods: vec!["POST".to_string()],
                            allow_headers: vec!["content-type".to_string()],
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                }),
            ],
        }),
        ..Default::default()
    }
}
