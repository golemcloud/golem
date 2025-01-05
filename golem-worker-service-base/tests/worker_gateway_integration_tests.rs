#[cfg(test)]
mod worker_gateway_integration_tests {
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use golem_service_base::auth::DefaultNamespace;
    use golem_common::model::{ComponentId, IdempotencyKey};
    use golem_service_base::model::{VersionedComponentId, Component, ComponentName};
    use golem_common::model::component_metadata::ComponentMetadata;
    use rib::{RibResult, RibByteCode, RibInput};
    use std::collections::HashMap;
    use serde_json::json;
    use poem_openapi::{
        OpenApi as PoemOpenApi, OpenApiService, ApiResponse, Object, payload::Json,
        param::Path,
    };
    use golem_worker_service_base::{
        gateway_api_definition::http::{
            HttpApiDefinition, CompiledHttpApiDefinition, HttpApiDefinitionRequest, 
            RouteRequest, MethodPattern, AllPathPatterns, ComponentMetadataDictionary,
        },
        gateway_binding::{
            GatewayBinding,
            worker_binding::WorkerBinding,
            worker_binding::ResponseMapping,
        },
        gateway_execution::{
            gateway_http_input_executor::{DefaultGatewayInputExecutor, GatewayHttpInputExecutor},
            gateway_session::{GatewaySession, DataKey, DataValue, SessionId, GatewaySessionError, GatewaySessionStore},
            api_definition_lookup::{ApiDefinitionsLookup, ApiDefinitionLookupError},
            file_server_binding_handler::{FileServerBindingHandler, FileServerBindingResult},
            gateway_binding_resolver::WorkerDetail,
        },
        gateway_request::{
            http_request::InputHttpRequest,
            request_details::HttpRequestDetails,
        },
        service::gateway::security_scheme::{SecuritySchemeService, SecuritySchemeServiceError},
        gateway_security::{SecurityScheme, IdentityProvider, Provider, IdentityProviderError, SecuritySchemeIdentifier, OpenIdClient, SecuritySchemeWithProviderMetadata},
        gateway_rib_interpreter::{WorkerServiceRibInterpreter, EvaluationError},
        gateway_api_definition::{ApiDefinitionId, ApiVersion},
    };
    use chrono::{DateTime, Utc};
    use tower::ServiceBuilder;
    use tower_http::trace::TraceLayer;
    use async_trait::async_trait;
    use axum::body::to_bytes;
    use openidconnect::{
        core::{
            CoreAuthDisplay, CoreClientAuthMethod, CoreClaimName, CoreClaimType,
            CoreGrantType, CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm,
            CoreJwsSigningAlgorithm, CoreGenderClaim, CoreJsonWebKeyType, CoreJsonWebKeyUse, CoreJsonWebKey,
            CoreResponseMode, CoreResponseType, CoreSubjectIdentifierType,
        },
        EmptyAdditionalClaims, EmptyAdditionalProviderMetadata,
        IdTokenFields, IdTokenClaims, ProviderMetadata,
        AuthorizationCode, IdTokenVerifier, Nonce,
    };
    use oauth2::{basic::BasicTokenType, Scope, CsrfToken, StandardTokenResponse, EmptyExtraTokenFields};
    use golem_worker_service_base::gateway_security::AuthorizationUrl;
    use golem_wasm_ast::analysis::{
        AnalysedType,
        TypeStr,
        TypeU32,
        TypeRecord,
        TypeList,
        NameTypePair,
        AnalysedExport,
        AnalysedFunction,
        AnalysedFunctionParameter,
        AnalysedFunctionResult,
        AnalysedInstance,
    };

    // Test component setup
    struct TestComponent;

    impl TestComponent {
        fn test_component_id() -> VersionedComponentId {
            VersionedComponentId {
                component_id: ComponentId::try_from("urn:uuid:550e8400-e29b-41d4-a716-446655440000").unwrap(),
                version: 0,
            }
        }
    }

    // Test API definition
    async fn create_test_api_definition() -> HttpApiDefinition {
        let create_at: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
        
        let request = HttpApiDefinitionRequest {
            id: ApiDefinitionId("test-api".to_string()),
            version: ApiVersion("1.0.0".to_string()),
            security: None,
            routes: vec![
                // Basic endpoints
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/healthcheck").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("healthcheck"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/version").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("version"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Primitive types demo
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/primitives").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("get-primitive-types"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/primitives").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("create-primitive-types"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // User management
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/users/{user-id}/profile").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("get-user-profile"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/users/{user-id}/settings").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("update-user-settings"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/users/{user-id}/permissions").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("get-user-permissions"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Content handling
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/content").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("create-content"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/content/{id}").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("get-content"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Search functionality
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/search").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("perform-search"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/search/validate").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("validate-search"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Batch operations
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/batch/process").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("batch-process"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/batch/validate").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("batch-validate"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/batch/{id}/status").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("get-batch-status"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Data transformations
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/transform").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("apply-transformation"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/transform/chain").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("chain-transformations"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Tree operations
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/tree").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("create-tree"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/tree/{id}").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("query-tree"),
                    }),
                    cors: None,
                    security: None,
                },
                RouteRequest {
                    method: MethodPattern::Post,
                    path: AllPathPatterns::parse("/tree/modify").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("modify-tree"),
                    }),
                    cors: None,
                    security: None,
                },
                
                // Export API definition
                RouteRequest {
                    method: MethodPattern::Get,
                    path: AllPathPatterns::parse("/v1/api/definitions/test-api/version/1.0.0/export").unwrap(),
                    binding: GatewayBinding::Default(WorkerBinding {
                        component_id: TestComponent::test_component_id(),
                        worker_name: None,
                        idempotency_key: None,
                        response_mapping: create_test_rib_mapping("export-api-definition"),
                    }),
                    cors: None,
                    security: None,
                },
            ],
            draft: true,
        };
        
        HttpApiDefinition::from_http_api_definition_request(
            &DefaultNamespace(),
            request,
            create_at,
            &test_utils::get_test_security_scheme_service(),
        )
        .await
        .unwrap()
    }

    // Helper function to create RIB mapping for each endpoint
    fn create_test_rib_mapping(function_name: &str) -> ResponseMapping {
        let rib_expr = match function_name {
            // No parameters
            "healthcheck" | "version" | "get-primitive-types" => format!(r#"
                let response = golem:it/api.{{{0}}}();
                response
            "#, function_name),
            
            // User profile endpoints - use user-id
            "get-user-profile" | "get-user-permissions" => format!(r#"
                let id: u32 = request.path.user-id;
                let response = golem:it/api.{{{0}}}(id);
                response
            "#, function_name),
            
            // Content and other endpoints - use id
            "get-content" | "get-batch-status" | "query-tree" => format!(r#"
                let id: u32 = request.path.id;
                let response = golem:it/api.{{{0}}}(id);
                response
            "#, function_name),
            
            // Only body parameter
            "create-primitive-types" | "create-content" | "perform-search" | "validate-search" | 
            "batch-process" | "batch-validate" | "create-tree" | "modify-tree" |
            "apply-transformation" | "chain-transformations" => format!(r#"
                let body = request.body;
                let response = golem:it/api.{{{0}}}(body);
                response
            "#, function_name),
            
            // Both path id and body
            "update-user-settings" => format!(r#"
                let id: u32 = request.path.user-id;
                let body = request.body;
                let response = golem:it/api.{{{0}}}(id, body);
                response
            "#, function_name),
            
            // Default case - no parameters
            _ => format!(r#"
                let response = golem:it/api.{{{0}}}();
                response
            "#, function_name),
        };
        println!("\nAttempting to parse RIB expression for {function_name}:\n{}", rib_expr);
        match rib::from_string(&rib_expr) {
            Ok(parsed) => {
                println!("Successfully parsed RIB expression for {function_name}");
                ResponseMapping(parsed)
            },
            Err(e) => {
                println!("Failed to parse RIB expression for {function_name}: {:?}", e);
                panic!("RIB parsing failed: {:?}", e);
            }
        }
    }

    // Helper function to create a test function with consistent structure
    fn create_test_function(name: &str) -> AnalysedFunction {
        let parameters = match name {
            // Basic endpoints - no parameters
            "healthcheck" | "version" => vec![],
            
            // Primitive types demo - body parameter for POST
            "get-primitive-types" => vec![],
            "create-primitive-types" => vec![
                AnalysedFunctionParameter {
                    name: "body".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            
            // User management - path id parameter and optional body
            "get-user-profile" | "get-user-permissions" => vec![
                AnalysedFunctionParameter {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
            "update-user-settings" => vec![
                AnalysedFunctionParameter {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                AnalysedFunctionParameter {
                    name: "settings".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            
            // Content handling - path id parameter and body
            "create-content" => vec![
                AnalysedFunctionParameter {
                    name: "body".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            "get-content" => vec![
                AnalysedFunctionParameter {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
            
            // Search functionality - body parameters
            "perform-search" | "validate-search" => vec![
                AnalysedFunctionParameter {
                    name: "body".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            
            // Batch operations - path id parameter and body
            "batch-process" | "batch-validate" => vec![
                AnalysedFunctionParameter {
                    name: "body".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            "get-batch-status" => vec![
                AnalysedFunctionParameter {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
            
            // Data transformations - body parameters
            "apply-transformation" | "chain-transformations" => vec![
                AnalysedFunctionParameter {
                    name: "body".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            
            // Tree operations - path id parameter and body
            "create-tree" | "modify-tree" => vec![
                AnalysedFunctionParameter {
                    name: "body".to_string(),
                    typ: AnalysedType::Record(TypeRecord { fields: vec![] }),
                },
            ],
            "query-tree" => vec![
                AnalysedFunctionParameter {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
            
            // Export API definition - no parameters
            "export-api-definition" => vec![],
            
            // Default case
            _ => vec![],
        };
        
        AnalysedFunction {
            name: name.to_string(),
            parameters,
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "component_id".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        NameTypePair {
                            name: "function_name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        NameTypePair {
                            name: "function_params".to_string(),
                            typ: AnalysedType::List(TypeList {
                                inner: Box::new(AnalysedType::Str(TypeStr)),
                            }),
                        },
                        NameTypePair {
                            name: "worker_name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                    ],
                }),
            }],
        }
    }

    // Define API response types
    #[derive(ApiResponse)]
    enum ApiError {
        /// Returns when there is an internal error
        #[oai(status = 500)]
        InternalError(Json<serde_json::Value>),
    }

    #[derive(ApiResponse)]
    enum ApiSuccess {
        /// Returns when the operation is successful
        #[oai(status = 200)]
        Ok(Json<serde_json::Value>),
    }

    // Define request/response objects
    #[derive(Object)]
    struct UserSettings {
        settings: serde_json::Value,
    }

    // Define the API
    struct TestApi;

    #[PoemOpenApi]
    impl TestApi {
        /// Health check endpoint
        #[oai(path = "/healthcheck", method = "get")]
        async fn healthcheck(&self) -> Result<ApiSuccess, ApiError> {
            Ok(ApiSuccess::Ok(Json(json!({
                "status": "ok",
                "version": "1.0.0"
            }))))
        }

        /// Version endpoint
        #[oai(path = "/version", method = "get")]
        async fn version(&self) -> Result<ApiSuccess, ApiError> {
            // Simulate an internal error scenario
            if rand::random::<bool>() {
                return Err(ApiError::InternalError(Json(json!({
                    "error": "Internal server error occurred while fetching version"
                }))));
            }
            Ok(ApiSuccess::Ok(Json(json!({
                "version": "1.0.0"
            }))))
        }

        /// Get user profile
        #[oai(path = "/users/:user_id/profile", method = "get")]
        async fn get_user_profile(
            &self,
            user_id: Path<i32>,
        ) -> Result<ApiSuccess, ApiError> {
            Ok(ApiSuccess::Ok(Json(json!({
                "id": user_id.0,
                "name": "Test User",
                "email": "test@example.com"
            }))))
        }

        /// Update user settings
        #[oai(path = "/users/:user_id/settings", method = "post")]
        async fn update_user_settings(
            &self,
            user_id: Path<i32>,
            settings: Json<UserSettings>,
        ) -> Result<ApiSuccess, ApiError> {
            Ok(ApiSuccess::Ok(Json(json!({
                "id": user_id.0,
                "settings": settings.0.settings,
                "updated": true
            }))))
        }
    }

    #[tokio::test]
    async fn test_worker_gateway_setup_and_api_serving() {
        // Create test API definition
        let api_definition = create_test_api_definition().await;
        println!("\nCreated test API definition: {:?}", api_definition);
        
        // Create test component with metadata
        let test_component = Component {
            versioned_component_id: TestComponent::test_component_id(),
            component_name: ComponentName("test-component".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![
                    AnalysedExport::Instance(AnalysedInstance {
                        name: "golem:it/api".to_string(),
                        functions: vec![
                            // Basic endpoints
                            create_test_function("healthcheck"),
                            create_test_function("version"),
                            
                            // Primitive types demo
                            create_test_function("get-primitive-types"),
                            create_test_function("create-primitive-types"),
                            
                            // User management
                            create_test_function("get-user-profile"),
                            create_test_function("update-user-settings"),
                            create_test_function("get-user-permissions"),
                            
                            // Content handling
                            create_test_function("create-content"),
                            create_test_function("get-content"),
                            
                            // Search functionality
                            create_test_function("perform-search"),
                            create_test_function("validate-search"),
                            
                            // Batch operations
                            create_test_function("batch-process"),
                            create_test_function("batch-validate"),
                            create_test_function("get-batch-status"),
                            
                            // Data transformations
                            create_test_function("apply-transformation"),
                            create_test_function("chain-transformations"),
                            
                            // Tree operations
                            create_test_function("create-tree"),
                            create_test_function("query-tree"),
                            create_test_function("modify-tree"),
                            
                            // Export API definition
                            create_test_function("export-api-definition"),
                        ],
                    }),
                ],
                producers: vec![],
                memories: vec![],
            },
            created_at: Some(chrono::Utc::now()),
            component_type: None,
            files: vec![],
            installed_plugins: vec![],
        };
        println!("\nCreated test component: {:?}", test_component);

        let mut metadata_dict = HashMap::new();
        metadata_dict.insert(test_component.versioned_component_id.clone(), test_component.metadata.exports.clone());
        let component_metadata = ComponentMetadataDictionary { metadata: metadata_dict };
        println!("\nRegistered component metadata: {:?}", component_metadata);

        let compiled_api_definition = CompiledHttpApiDefinition::from_http_api_definition(
            &api_definition,
            &component_metadata,
            &DefaultNamespace(),
        ).unwrap();
        println!("\nCompiled API definition: {:?}", compiled_api_definition);

        // Create OpenAPI service and store it to be used later
        let _api_service = OpenApiService::new(TestApi, "Test API", "1.0.0")
            .server("http://localhost:8000")
            .description("Test API for Worker Gateway")
            .external_document("https://example.com/docs");
        
        // Create and bind TCP listener for the Worker Gateway
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        println!("\nWorker Gateway listening on {}", addr);

        // Set up session store
        let session_store = test_utils::get_test_session_store();
        println!("\nInitialized session store");

        // Set up identity provider
        let identity_provider = Arc::new(test_utils::TestIdentityProvider::default());
        println!("\nInitialized identity provider");

        // Set up API lookup service
        let api_lookup = Arc::new(test_utils::TestApiDefinitionLookup::new(compiled_api_definition.clone()));
        println!("\nInitialized API lookup service with compiled definition");
        
        // Set up file server handler
        let file_server_handler = test_utils::get_test_file_server_binding_handler();
        println!("\nInitialized file server handler");

        // Create the Worker Gateway executor
        let executor = DefaultGatewayInputExecutor::new(
            api_lookup.clone(),
            file_server_handler,
            Arc::new(test_utils::TestAuthCallBackHandler::default()),
            api_lookup.clone(),
            session_store.clone(),
            identity_provider.clone(),
        );
        println!("\nCreated Worker Gateway executor");

        // Create the HTTP router with tracing
        let executor = Arc::new(executor) as Arc<dyn GatewayHttpInputExecutor + Send + Sync>;
        let app = axum::Router::new()
            .fallback(move |req: axum::http::Request<axum::body::Body>| {
                let executor = Arc::clone(&executor);
                async move {
                    // Convert axum request to poem request
                    let (parts, body) = req.into_parts();
                    println!("\nIncoming request: {} {}", parts.method, parts.uri);
                    println!("Request headers: {:?}", parts.headers);

                    let body_bytes = to_bytes(body, usize::MAX).await.unwrap();
                    println!("Request body: {}", String::from_utf8_lossy(&body_bytes));

                    let mut builder = poem::Request::builder()
                        .method(parts.method)
                        .uri(parts.uri)
                        .version(parts.version);

                    // Add headers
                    builder = builder.header("Host", "localhost");
                    builder = builder.header("Content-Type", "application/json");
                    
                    // Add other headers from original request
                    for (key, value) in parts.headers.iter() {
                        if key.as_str().to_lowercase() != "host" {  // Skip host header as we set it above
                            builder = builder.header(key.as_str(), value.to_str().unwrap_or_default());
                        }
                    }

                    let poem_req = builder.body(poem::Body::from(body_bytes));
                    println!("Converted to poem request with headers: {:?}", poem_req.headers());

                    // Execute request through gateway
                    let response = executor.execute_http_request(poem_req).await;
                    println!("\nGateway executor processed request");
                    
                    // Convert poem response to axum response
                    let (parts, body) = response.into_parts();
                    println!("Response status: {:?}", parts.status);
                    println!("Response headers: {:?}", parts.headers);

                    let body_bytes = body.into_bytes().await.unwrap();
                    println!("Response body: {}", String::from_utf8_lossy(&body_bytes));

                    let body = if body_bytes.is_empty() {
                        axum::body::Body::empty()
                    } else {
                        axum::body::Body::from(body_bytes)
                    };
                    
                    let mut builder = axum::http::Response::builder()
                        .status(parts.status)
                        .version(parts.version);
                    
                    for (key, value) in parts.headers.iter() {
                        builder = builder.header(key, value);
                    }
                    
                    builder.body(body).unwrap()
                }
            })
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()));
        println!("\nCreated HTTP router with tracing");

        // Spawn the Worker Gateway server
        println!("\nSpawning Worker Gateway server...");
        let server_handle = tokio::spawn(async move {
            println!("\nWorker Gateway server starting...");
            axum::serve(
                listener,
                app.into_make_service(),
            )
            .await
            .unwrap();
        });
        println!("\nWorker Gateway server spawned");

        // Give the server a moment to start up
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Create test HTTP request
        let client = reqwest::Client::new();
        println!("\nSending test request to http://{}/test", addr);
        
        // First verify the server is up with a health check
        let health_response = client
            .get(&format!("http://{}/healthcheck", addr))
            .header("Accept", "application/json")
            .send()
            .await;
        
        match health_response {
            Ok(resp) => {
                println!("\nHealth check response: {:?}", resp.status());
                if !resp.status().is_success() {
                    panic!("Health check failed: {}", resp.status());
                }
            }
            Err(e) => {
                panic!("Health check failed: {}", e);
            }
        }
        println!("\nHealth check passed, server is up");

        // Test all endpoints
        let test_cases = vec![
            // Basic endpoints
            ("/healthcheck", "GET"),
            ("/version", "GET"),
            
            // Primitive types demo
            ("/primitives", "GET"),
            ("/primitives", "POST"),
            
            // User management
            ("/users/1/profile", "GET"),
            ("/users/1/settings", "POST"),
            ("/users/1/permissions", "GET"),
            
            // Content handling
            ("/content", "POST"),
            ("/content/1", "GET"),
            
            // Search functionality
            ("/search", "POST"),
            ("/search/validate", "POST"),
            
            // Batch operations
            ("/batch/process", "POST"),
            ("/batch/validate", "POST"),
            ("/batch/1/status", "GET"),
            
            // Data transformations
            ("/transform", "POST"),
            ("/transform/chain", "POST"),
            
            // Tree operations
            ("/tree", "POST"),
            ("/tree/1", "GET"),
            ("/tree/modify", "POST"),
            
            // Export API definition
            ("/v1/api/definitions/test-api/version/1.0.0/export", "GET"),
        ];

        for (path, method) in test_cases {
            println!("\nTesting {} {}", method, path);
            let request = match method {
                "GET" => client.get(&format!("http://{}{}", addr, path)),
                "POST" => {
                    let test_body = json!({
                        "test": "data"
                    });
                    client.post(&format!("http://{}{}", addr, path)).json(&test_body)
                },
                _ => panic!("Unsupported method: {}", method),
            }
            .header("Host", "localhost")
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");

            let response = request.send().await.unwrap();
            
            let status = response.status();
            println!("Response status: {}", status);
            println!("Response headers: {:?}", response.headers());
            
            let response_text = response.text().await.unwrap();
            println!("Response body: {}", response_text);
            
            assert!(
                status.is_success(),
                "Expected successful response from {} {}, got {} with body: {}",
                method,
                path,
                status,
                response_text
            );
            
            // Verify response structure if it's JSON
            if let Ok(response_json) = serde_json::from_str::<serde_json::Value>(&response_text) {
                assert!(response_json.get("status").is_some(), "Response should have status field");
                assert!(response_json.get("data").is_some(), "Response should have data field");
            }
        }

        println!("\nAll response validations passed");

        // Cleanup
        println!("\nShutting down Worker Gateway server...");
        server_handle.abort();
        println!("Worker Gateway server shutdown complete");
    }

    mod test_utils {
        use super::*;
        use golem_worker_service_base::gateway_execution::auth_call_back_binding_handler::{
            AuthCallBackResult, AuthorisationSuccess, AuthCallBackBindingHandler,
        };

        pub struct TestSessionStore {
            sessions: Arc<tokio::sync::Mutex<HashMap<SessionId, HashMap<DataKey, DataValue>>>>,
        }

        #[async_trait]
        impl GatewaySession for TestSessionStore {
            async fn get(&self, session_id: &SessionId, key: &DataKey) -> Result<DataValue, GatewaySessionError> {
                let sessions = self.sessions.lock().await;
                sessions
                    .get(session_id)
                    .and_then(|s| s.get(key))
                    .cloned()
                    .ok_or(GatewaySessionError::InternalError("Session data not found".to_string()))
            }

            async fn insert(&self, session_id: SessionId, key: DataKey, value: DataValue) -> Result<(), GatewaySessionError> {
                let mut sessions = self.sessions.lock().await;
                let session = sessions.entry(session_id).or_insert_with(HashMap::new);
                session.insert(key, value);
                Ok(())
            }
        }

        pub fn get_test_session_store() -> Arc<dyn GatewaySession + Send + Sync> {
            Arc::new(TestSessionStore {
                sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            })
        }

        pub struct TestIdentityProvider;

        impl Default for TestIdentityProvider {
            fn default() -> Self {
                Self
            }
        }

        #[async_trait]
        impl IdentityProvider for TestIdentityProvider {
            async fn get_provider_metadata(&self, _provider: &Provider) -> Result<ProviderMetadata<
                EmptyAdditionalProviderMetadata,
                CoreAuthDisplay,
                CoreClientAuthMethod,
                CoreClaimName,
                CoreClaimType,
                CoreGrantType,
                CoreJweContentEncryptionAlgorithm,
                CoreJweKeyManagementAlgorithm,
                CoreJwsSigningAlgorithm,
                CoreJsonWebKeyType,
                CoreJsonWebKeyUse,
                CoreJsonWebKey,
                CoreResponseMode,
                CoreResponseType,
                CoreSubjectIdentifierType
            >, IdentityProviderError> {
                unimplemented!()
            }

            async fn exchange_code_for_tokens(
                &self,
                _client: &OpenIdClient,
                _code: &AuthorizationCode,
            ) -> Result<StandardTokenResponse<IdTokenFields<EmptyAdditionalClaims, EmptyExtraTokenFields, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm, CoreJsonWebKeyType>, BasicTokenType>, IdentityProviderError> {
                unimplemented!()
            }

            async fn get_client(
                &self,
                _scheme: &SecurityScheme,
            ) -> Result<OpenIdClient, IdentityProviderError> {
                unimplemented!()
            }

            fn get_id_token_verifier<'a>(
                &self,
                _client: &'a OpenIdClient,
            ) -> IdTokenVerifier<'a, CoreJwsSigningAlgorithm, CoreJsonWebKeyType, CoreJsonWebKeyUse, CoreJsonWebKey> {
                unimplemented!()
            }

            fn get_claims(
                &self,
                _verifier: &IdTokenVerifier<'_, CoreJwsSigningAlgorithm, CoreJsonWebKeyType, CoreJsonWebKeyUse, CoreJsonWebKey>,
                _token_response: StandardTokenResponse<IdTokenFields<EmptyAdditionalClaims, EmptyExtraTokenFields, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm, CoreJsonWebKeyType>, BasicTokenType>,
                _nonce: &Nonce,
            ) -> Result<IdTokenClaims<EmptyAdditionalClaims, CoreGenderClaim>, IdentityProviderError> {
                unimplemented!()
            }

            fn get_authorization_url(
                &self,
                _client: &OpenIdClient,
                _scopes: Vec<Scope>,
                _csrf_token: Option<CsrfToken>,
                _nonce: Option<Nonce>,
            ) -> AuthorizationUrl {
                unimplemented!()
            }
        }

        pub struct TestSecuritySchemeService;

        #[async_trait]
        impl SecuritySchemeService<DefaultNamespace> for TestSecuritySchemeService {
            async fn get(&self, _id: &SecuritySchemeIdentifier, _namespace: &DefaultNamespace) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
                unimplemented!()
            }

            async fn create(&self, _namespace: &DefaultNamespace, _scheme: &SecurityScheme) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
                unimplemented!()
            }
        }

        pub fn get_test_security_scheme_service() -> Arc<dyn SecuritySchemeService<DefaultNamespace> + Send + Sync> {
            Arc::new(TestSecuritySchemeService)
        }

        pub struct TestFileServerBindingHandler;

        #[async_trait]
        impl FileServerBindingHandler<DefaultNamespace> for TestFileServerBindingHandler {
            async fn handle_file_server_binding_result(
                &self,
                _namespace: &DefaultNamespace,
                _worker_detail: &WorkerDetail,
                _original_result: RibResult,
            ) -> FileServerBindingResult {
                unimplemented!()
            }
        }

        pub fn get_test_file_server_binding_handler() -> Arc<dyn FileServerBindingHandler<DefaultNamespace> + Send + Sync> {
            Arc::new(TestFileServerBindingHandler)
        }

        pub struct TestApiDefinitionLookup {
            api_definition: CompiledHttpApiDefinition<DefaultNamespace>,
        }

        impl TestApiDefinitionLookup {
            pub fn new(api_definition: CompiledHttpApiDefinition<DefaultNamespace>) -> Self {
                println!("\nCreating TestApiDefinitionLookup with routes:");
                for route in &api_definition.routes {
                    println!("  {} {}", route.method, route.path);
                }
                Self { api_definition }
            }
        }

        #[async_trait]
        impl ApiDefinitionsLookup<InputHttpRequest> for TestApiDefinitionLookup {
            type ApiDefinition = CompiledHttpApiDefinition<DefaultNamespace>;

            async fn get(
                &self,
                input: &InputHttpRequest,
            ) -> Result<Vec<Self::ApiDefinition>, ApiDefinitionLookupError> {
                println!("\nAPI Lookup called for {} {}", input.req_method, input.api_input_path.base_path);
                println!("Available routes:");
                for route in &self.api_definition.routes {
                    println!("  {} {}", route.method, route.path);
                }
                Ok(vec![self.api_definition.clone()])
            }
        }

        #[async_trait]
        impl<Namespace> WorkerServiceRibInterpreter<Namespace> for TestApiDefinitionLookup
        where
            Namespace: Send + Sync + 'static,
        {
            async fn evaluate(
                &self,
                worker_name: Option<&str>,
                component_id: &ComponentId,
                idempotency_key: &Option<IdempotencyKey>,
                _rib_byte_code: &RibByteCode,
                rib_input: &RibInput,
                _namespace: Namespace,
            ) -> Result<RibResult, EvaluationError> {
                use golem_wasm_rpc::{Value, ValueAndType};
                use golem_wasm_ast::analysis::{AnalysedType, TypeRecord, NameTypePair, TypeStr, TypeList, TypeU32};
                
                println!("\nRIB Interpreter evaluating request:");
                println!("  Worker name: {:?}", worker_name);
                println!("  Component ID: {:?}", component_id);
                println!("  Idempotency key: {:?}", idempotency_key);
                println!("  RIB input: {:?}", rib_input);
                
                // Create a mock response based on the request
                let response_record = vec![
                    Value::String(component_id.0.to_string()),
                    Value::String("test-function".to_string()),
                    Value::List(vec![
                        Value::String("param1".to_string()),
                        Value::String("param2".to_string()),
                    ]),
                    Value::String(worker_name.unwrap_or("default-worker").to_string()),
                ];

                let response_type = AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "component_id".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        NameTypePair {
                            name: "function_name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        NameTypePair {
                            name: "function_params".to_string(),
                            typ: AnalysedType::List(TypeList {
                                inner: Box::new(AnalysedType::Str(TypeStr)),
                            }),
                        },
                        NameTypePair {
                            name: "worker_name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                    ],
                });

                // Create the final response with status and data
                let result = RibResult::Val(ValueAndType::new(
                    Value::Record(vec![
                        Value::U32(200),  // status field as number (HTTP 200 OK)
                        Value::Record(response_record),        // data field
                    ]),
                    AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "status".to_string(),
                                typ: AnalysedType::U32(TypeU32),  // Change type to U32
                            },
                            NameTypePair {
                                name: "data".to_string(),
                                typ: response_type,
                            },
                        ],
                    }),
                ));

                println!("  Generated RIB result: {:?}", result);
                Ok(result)
            }
        }

        pub struct TestAuthCallBackHandler;

        impl Default for TestAuthCallBackHandler {
            fn default() -> Self {
                Self
            }
        }

        #[async_trait]
        impl AuthCallBackBindingHandler for TestAuthCallBackHandler {
            async fn handle_auth_call_back(
                &self,
                _http_request_details: &HttpRequestDetails,
                _security_scheme: &SecuritySchemeWithProviderMetadata,
                _gateway_session_store: &GatewaySessionStore,
                _identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
            ) -> AuthCallBackResult {
                use oauth2::AccessToken;
                use openidconnect::EmptyExtraTokenFields;

                Ok(AuthorisationSuccess {
                    token_response: StandardTokenResponse::new(
                        AccessToken::new("test-access-token".to_string()),
                        BasicTokenType::Bearer,
                        IdTokenFields::new(None, EmptyExtraTokenFields {}),
                    ),
                    target_path: "/".to_string(),
                    id_token: None,
                    access_token: "test-token".to_string(),
                    session: "test-session".to_string(),
                })
            }
        }
    }
} 