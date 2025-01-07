use anyhow::Result;
use golem_worker_service_base::{
    gateway_api_definition::http::swagger_ui::{
        SwaggerUiConfig, SwaggerUiWorkerBinding, SwaggerUiAuthConfig,
    },
    service::worker::{
        default::{WorkerService, WorkerResult, WorkerRequestMetadata},
        WorkerStream, WorkerServiceError,
    },
    api::routes::create_api_router,
    repo::{
        api_definition::{ApiDefinitionRepo, ApiDefinitionRecord},
        api_deployment::{ApiDeploymentRepo, ApiDeploymentRecord},
    },
    service::gateway::{
        security_scheme::{SecuritySchemeService, SecuritySchemeServiceError},
        api_definition_validator::{ApiDefinitionValidatorService, ValidationErrors},
    },
    gateway_api_definition::http::HttpApiDefinition,
    service::component::{ComponentService, ComponentResult},
    gateway_security::{
        SecurityScheme, SecuritySchemeWithProviderMetadata, SecuritySchemeIdentifier,
        Provider,
    },
};
use golem_common::model::{
    ComponentId, WorkerId, TargetWorkerId, ComponentFilePath, ComponentFileSystemNode,
    WorkerFilter, ScanCursor, ComponentVersion, PluginInstallationId, Timestamp,
    component_metadata::ComponentMetadata, component_constraint::FunctionConstraintCollection,
};
use golem_service_base::{
    auth::DefaultNamespace,
    model::{WorkerMetadata, GetOplogResponse, Component, ComponentName, VersionedComponentId},
};
use golem_common::model::public_oplog::OplogCursor;
use golem_common::model::oplog::OplogIndex;
use golem_api_grpc::proto::golem::worker::{UpdateMode, InvocationContext, LogEvent};
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use poem::test::TestClient;
use poem_openapi::{
    Object,
    Tags,
    OpenApi,
    param::Path,
    payload::Json as PoemJson,
    OpenApiService,
};
use std::{collections::{HashMap, HashSet}, sync::Arc, pin::Pin};
use async_trait::async_trait;
use futures::Stream;
use bytes::Bytes;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Streaming, codec::{ProstCodec, Codec}};
use http_body_util::Full;
use openidconnect::{
    ClientId, ClientSecret, RedirectUrl, Scope,
    JsonWebKeySetUrl, IssuerUrl, AuthUrl, ResponseTypes,
};
use openidconnect::core::{
    CoreResponseType, CoreSubjectIdentifierType, CoreJwsSigningAlgorithm, CoreProviderMetadata,
};
use golem_service_base::repo::RepoError;
use poem::Route;

test_r::enable!();

// Test API structures
#[derive(Debug, Object, Clone, serde::Serialize, serde::Deserialize)]
struct DynamicRequest {
    message: String,
}

#[derive(Debug, Object, Clone, serde::Serialize, serde::Deserialize)]
struct DynamicResponse {
    result: String,
}

#[derive(Debug, Object, Clone, serde::Serialize, serde::Deserialize)]
struct DynamicEndpoint {
    path: String,
    method: String,
    description: String,
}

#[derive(Tags)]
enum ApiTags {
    Dynamic
}

#[derive(Clone)]
struct TestApi;

// Mock Worker Service
#[derive(Default)]
struct MockWorkerService {
    registered_workers: Arc<tokio::sync::RwLock<HashMap<WorkerId, u64>>>,
}

// Helper functions for creating test data
fn create_test_worker_metadata() -> WorkerMetadata {
    WorkerMetadata {
        worker_id: WorkerId {
            component_id: ComponentId::new_v4(),
            worker_name: "test-worker".to_string(),
        },
        args: Vec::new(),
        env: HashMap::new(),
        status: golem_common::model::WorkerStatus::Running,
        component_version: 0,
        retry_count: 0,
        pending_invocation_count: 0,
        updates: Vec::new(),
        created_at: Timestamp::now_utc(),
        last_error: None,
        component_size: 0,
        total_linear_memory_size: 0,
        owned_resources: HashMap::new(),
        active_plugins: HashSet::new(),
    }
}

fn create_test_oplog_response() -> GetOplogResponse {
    GetOplogResponse {
        entries: Vec::new(),
        next: None,
        first_index_in_chunk: 0,
        last_index: 0,
    }
}

#[async_trait]
impl WorkerService for MockWorkerService {
    async fn get_swagger_ui_contents(
        &self,
        _worker_id: &TargetWorkerId,
        _mount_path: String,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let bytes = Bytes::from("mock swagger ui contents");
        let stream = futures::stream::once(async move { Ok(bytes) });
        Ok(Box::pin(stream))
    }

    async fn get_file_contents(
        &self,
        _worker_id: &TargetWorkerId,
        _path: ComponentFilePath,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let bytes = Bytes::from("mock file contents");
        let stream = futures::stream::once(async move { Ok(bytes) });
        Ok(Box::pin(stream))
    }

    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: u64,
        _arguments: Vec<String>,
        _environment_variables: HashMap<String, String>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<WorkerId> {
        let mut registered_workers = self.registered_workers.write().await;
        registered_workers.insert(worker_id.clone(), component_version);
        Ok(worker_id.clone())
    }

    async fn connect(
        &self,
        _worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<WorkerStream<LogEvent>> {
        // Create a channel for streaming log events
        let (_tx, rx) = tokio::sync::mpsc::channel::<LogEvent>(32);
        
        // Convert the receiver into a stream
        let _stream = ReceiverStream::new(rx);
        
        // Create a body for streaming
        let body = Full::new(Bytes::new());
        
        // Create a codec and get its decoder
        let mut codec = ProstCodec::<LogEvent, LogEvent>::default();
        let decoder = Codec::decoder(&mut codec);
        
        // Create a tonic::Streaming using new_response
        let streaming = Streaming::new_response(
            decoder,
            body,
            http::StatusCode::OK,
            Default::default(),
            None
        );
        
        // Create WorkerStream with the Streaming type
        Ok(WorkerStream::new(streaming))
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        let mut registered_workers = self.registered_workers.write().await;
        registered_workers.remove(worker_id);
        Ok(())
    }

    fn validate_typed_parameters(
        &self,
        _params: Vec<golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue>,
    ) -> WorkerResult<Vec<ProtoVal>> {
        // Return empty vector for testing
        Ok(vec![])
    }

    async fn get_metadata(
        &self,
        _worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<WorkerMetadata> {
        Ok(create_test_worker_metadata())
    }

    async fn find_metadata(
        &self,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
        _cursor: ScanCursor,
        _count: u64,
        _precise: bool,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadata>)> {
        // Return empty result for testing
        Ok((None, vec![]))
    }

    async fn resume(
        &self,
        _worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn update(
        &self,
        _worker_id: &WorkerId,
        _update_mode: UpdateMode,
        _target_version: ComponentVersion,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn get_oplog(
        &self,
        _worker_id: &WorkerId,
        _from_oplog_index: OplogIndex,
        _cursor: Option<OplogCursor>,
        _count: u64,
        _metadata: WorkerRequestMetadata,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        Ok(create_test_oplog_response())
    }

    async fn search_oplog(
        &self,
        _worker_id: &WorkerId,
        _cursor: Option<OplogCursor>,
        _count: u64,
        _query: String,
        _metadata: WorkerRequestMetadata,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        Ok(create_default_oplog_response())
    }

    async fn list_directory(
        &self,
        _worker_id: &TargetWorkerId,
        _path: ComponentFilePath,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        Ok(vec![])
    }

    async fn activate_plugin(
        &self,
        _worker_id: &WorkerId,
        _plugin_installation_id: &PluginInstallationId,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn deactivate_plugin(
        &self,
        _worker_id: &WorkerId,
        _plugin_installation_id: &PluginInstallationId,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn invoke_and_await_typed(
        &self,
        _worker_id: &TargetWorkerId,
        _idempotency_key: Option<golem_common::model::IdempotencyKey>,
        _function_name: String,
        _arguments: Vec<ProtoVal>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue> {
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
        Ok(TypeAnnotatedValue::Option(Box::new(golem_wasm_rpc::protobuf::TypedOption {
            typ: None,
            value: None,
        })))
    }

    async fn invoke_and_await(
        &self,
        _worker_id: &TargetWorkerId,
        _idempotency_key: Option<golem_common::model::IdempotencyKey>,
        _function_name: String,
        _arguments: Vec<golem_wasm_rpc::protobuf::Val>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<golem_api_grpc::proto::golem::worker::InvokeResult> {
        // Return empty InvokeResult for testing
        Ok(golem_api_grpc::proto::golem::worker::InvokeResult::default())
    }

    async fn invoke(
        &self,
        _worker_id: &TargetWorkerId,
        _idempotency_key: Option<golem_common::model::IdempotencyKey>,
        _function_name: String,
        _arguments: Vec<golem_wasm_rpc::protobuf::Val>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn complete_promise(
        &self,
        _worker_id: &WorkerId,
        _promise_id: u64,
        _result: Vec<u8>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<bool> {
        Ok(true)
    }

    async fn interrupt(
        &self,
        _worker_id: &WorkerId,
        _force: bool,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }
}

async fn setup_mock_worker_service() -> Arc<dyn WorkerService + Send + Sync> {
    Arc::new(MockWorkerService::default())
}

#[derive(Default)]
struct MockComponentService;

#[async_trait]
impl ComponentService<DefaultNamespace> for MockComponentService {
    async fn get_by_version(
        &self,
        _component_id: &ComponentId,
        _version: u64,
        _namespace: &DefaultNamespace,
    ) -> ComponentResult<Component> {
        Ok(Component {
            versioned_component_id: VersionedComponentId {
                component_id: ComponentId::try_from("test-id").unwrap(),
                version: 0,
            },
            component_name: ComponentName("test".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
                memories: vec![],
                dynamic_linking: HashMap::new(),
            },
            created_at: Some(chrono::Utc::now()),
            component_type: None,
            files: vec![],
            installed_plugins: vec![],
        })
    }

    async fn get_latest(
        &self,
        _component_id: &ComponentId,
        _namespace: &DefaultNamespace,
    ) -> ComponentResult<Component> {
        Ok(Component {
            versioned_component_id: VersionedComponentId {
                component_id: ComponentId::try_from("test-id").unwrap(),
                version: 0,
            },
            component_name: ComponentName("test".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
                memories: vec![],
                dynamic_linking: HashMap::new(),
            },
            created_at: Some(chrono::Utc::now()),
            component_type: None,
            files: vec![],
            installed_plugins: vec![],
        })
    }

    async fn create_or_update_constraints(
        &self,
        _component_id: &ComponentId,
        constraints: FunctionConstraintCollection,
        _namespace: &DefaultNamespace,
    ) -> ComponentResult<FunctionConstraintCollection> {
        Ok(constraints)
    }
}

async fn setup_mock_component_service() -> Arc<dyn ComponentService<DefaultNamespace> + Send + Sync> {
    Arc::new(MockComponentService::default())
}

#[tokio::test]
async fn test_dynamic_swagger_ui_worker_binding() -> Result<()> {
    println!("\n=== Starting test_dynamic_swagger_ui_worker_binding ===");
    
    // 1. Set up mock worker service
    let _worker_service = setup_mock_worker_service().await;
    let component_service = setup_mock_component_service().await;
    
    // 2. Create SwaggerUI binding
    let worker_binding = SwaggerUiWorkerBinding {
        worker_name: "test-api-worker".to_string(),
        component_id: ComponentId::new_v4().to_string(),
        component_version: Some(1),
        mount_path: "/test-api".to_string(),
    };
    println!("Worker binding configured with mount_path: {}", worker_binding.mount_path);

    let config = SwaggerUiConfig {
        enabled: true,
        title: Some("Test API".to_string()),
        version: Some("1.0".to_string()),
        server_url: Some(format!("http://localhost:{}", 9005)),
        auth: SwaggerUiAuthConfig::default(),
        worker_binding: Some(worker_binding),
        golem_extensions: HashMap::new(),
    };

    // 3. Create API service with SwaggerUI and dynamic routes
    let api_service = OpenApiService::new(TestApi, "Test API", "1.0.0")
        .server("/test-api".to_string());

    let swagger_ui = api_service.swagger_ui();
    let spec_endpoint = api_service.spec_endpoint();

    let base_app = create_api_router(
        Some("http://localhost:9005".to_string()),
        component_service.clone(),
        Arc::new(MockApiDefinitionRepo),
        Arc::new(MockApiDeploymentRepo),
        Arc::new(MockSecuritySchemeService),
        Arc::new(MockApiDefinitionValidator),
        Some(config.clone()),
    ).await?;

    // Create a new Route and combine everything
    let app = Route::new()
        .nest("/", base_app)
        .nest("/test-api", api_service)
        .nest("/test-api/docs", swagger_ui)
        .at("/test-api/openapi.json", spec_endpoint);

    let client = TestClient::new(app);

    // 4. Test dynamic endpoints
    println!("\nTesting /test-api/test endpoint:");
    let resp = client.get("/test-api/test").send().await;
    let status = resp.0.status();
    println!("  Status: {}", status);
    let test_body = String::from_utf8(resp.0.into_body().into_bytes().await?.to_vec())?;
    println!("  Response body: {}", test_body);
    assert_eq!(status.as_u16(), 200, "GET /test-api/test failed");
    
    println!("\nTesting /test-api/dynamic/test-path endpoint:");
    let test_req = DynamicRequest {
        message: "test message".to_string(),
    };
    
    let resp = client
        .post("/test-api/dynamic/test-path")
        .body_json(&test_req)
        .send()
        .await;
    
    let status = resp.0.status();
    println!("  Status: {}", status);
    let dynamic_body = String::from_utf8(resp.0.into_body().into_bytes().await?.to_vec())?;
    println!("  Response body: {}", dynamic_body);
    assert_eq!(status.as_u16(), 200, "POST /test-api/dynamic/test-path failed");

    // 5. Test SwaggerUI page existence and content
    println!("\nTesting SwaggerUI endpoint:");
    let resp = client.get("/test-api/docs/").send().await;
    let status = resp.0.status();
    println!("  Status: {}", status);
    let swagger_ui_content = String::from_utf8(resp.0.into_body().into_bytes().await?.to_vec())?;
    println!("  Response length: {} bytes", swagger_ui_content.len());
    println!("  Contains 'swagger-ui': {}", swagger_ui_content.contains("swagger-ui"));
    assert_eq!(status.as_u16(), 200, "GET /test-api/docs/ failed");

    // 6. Test SwaggerUI JSON/YAML spec endpoint
    println!("\nTesting OpenAPI spec endpoint:");
    let resp = client.get("/test-api/openapi.json").send().await;
    let status = resp.0.status();
    println!("  Status: {}", status);
    let spec_content = String::from_utf8(resp.0.into_body().into_bytes().await?.to_vec())?;
    println!("  Response length: {} bytes", spec_content.len());
    println!("  Contains 'openapi': {}", spec_content.contains("openapi"));
    assert_eq!(status.as_u16(), 200, "GET /test-api/openapi.json failed");

    println!("=== Test completed successfully ===\n");
    Ok(())
}

#[tokio::test]
async fn test_dynamic_swagger_ui_path_binding() -> Result<()> {
    println!("\n=== Starting test_dynamic_swagger_ui_path_binding ===");
    
    // 1. Set up mock services
    let _worker_service = setup_mock_worker_service().await;
    let component_service = setup_mock_component_service().await;
    
    // 2. Test multiple SwaggerUI bindings with different paths
    let test_paths = vec![
        "/custom/path1".to_string(),
        "/custom/path2".to_string(),
        "/custom/path3".to_string(),
    ];

    for mount_path in &test_paths {
        println!("\nTesting mount path: {}", mount_path);
        
        let worker_binding = SwaggerUiWorkerBinding {
            worker_name: format!("test-worker-{}", mount_path.replace("/", "-")),
            component_id: ComponentId::new_v4().to_string(),
            component_version: Some(1),
            mount_path: mount_path.clone(),
        };

        let config = SwaggerUiConfig {
            enabled: true,
            title: Some(format!("Test API at {}", mount_path)),
            version: Some("1.0".to_string()),
            server_url: Some(format!("http://localhost:{}", 9005)),
            auth: SwaggerUiAuthConfig::default(),
            worker_binding: Some(worker_binding.clone()),
            golem_extensions: HashMap::new(),
        };

        // 3. Create API service with SwaggerUI
        let api_service = OpenApiService::new(
            TestApi,
            format!("Test API at {}", mount_path),
            "1.0.0"
        )
        .server(mount_path.clone());

        let swagger_ui = api_service.swagger_ui();
        let spec_endpoint = api_service.spec_endpoint();

        let base_app = create_api_router(
            Some("http://localhost:9005".to_string()),
            component_service.clone(),
            Arc::new(MockApiDefinitionRepo),
            Arc::new(MockApiDeploymentRepo),
            Arc::new(MockSecuritySchemeService),
            Arc::new(MockApiDefinitionValidator),
            Some(config.clone()),
        ).await?;

        let app = Route::new()
            .nest("/", base_app)
            .nest(mount_path, api_service)
            .nest(&format!("{}/docs", mount_path), swagger_ui)
            .at(&format!("{}/openapi.json", mount_path), spec_endpoint);

        let client = TestClient::new(app);

        // 4. Test SwaggerUI existence at custom path
        println!("Testing SwaggerUI at {}/docs/", mount_path);
        let resp = client.get(&format!("{}/docs/", mount_path)).send().await;
        let status = resp.0.status();
        println!("  Status: {}", status);
        let swagger_content = String::from_utf8(resp.0.into_body().into_bytes().await?.to_vec())?;
        println!("  Response length: {} bytes", swagger_content.len());
        assert_eq!(status.as_u16(), 200, "GET {}/docs/ failed", mount_path);

        // 5. Test API endpoints at custom path
        println!("Testing API endpoint at {}/test", mount_path);
        let resp = client.get(&format!("{}/test", mount_path)).send().await;
        let status = resp.0.status();
        println!("  Status: {}", status);
        let test_content = String::from_utf8(resp.0.into_body().into_bytes().await?.to_vec())?;
        println!("  Response body: {}", test_content);
        assert_eq!(status.as_u16(), 200, "GET {}/test failed", mount_path);
    }

    println!("=== Test completed successfully ===\n");
    Ok(())
}

#[tokio::test]
async fn test_swagger_ui_export() -> Result<()> {
    use tokio::fs;
    use std::path::PathBuf;

    println!("\n=== Starting test_swagger_ui_export ===");

    // 1. Set up mock services and SwaggerUI
    let _worker_service = setup_mock_worker_service().await;
    let component_service = setup_mock_component_service().await;
    
    let worker_binding = SwaggerUiWorkerBinding {
        worker_name: "export-test-worker".to_string(),
        component_id: ComponentId::new_v4().to_string(),
        component_version: Some(1),
        mount_path: "/export-api".to_string(),
    };
    println!("Worker binding configured with mount_path: {}", worker_binding.mount_path);

    let config = SwaggerUiConfig {
        enabled: true,
        title: Some("Export Test API".to_string()),
        version: Some("1.0".to_string()),
        server_url: Some("http://localhost:9005".to_string()),
        auth: SwaggerUiAuthConfig::default(),
        worker_binding: Some(worker_binding.clone()),
        golem_extensions: HashMap::new(),
    };

    // 2. Create API service
    let api_service = OpenApiService::new(TestApi, "Export Test API", "1.0.0")
        .server("/export-api".to_string());

    let swagger_ui = api_service.swagger_ui();
    let spec_endpoint = api_service.spec_endpoint();

    let base_app = create_api_router(
        Some("http://localhost:9005".to_string()),
        component_service.clone(),
        Arc::new(MockApiDefinitionRepo),
        Arc::new(MockApiDeploymentRepo),
        Arc::new(MockSecuritySchemeService),
        Arc::new(MockApiDefinitionValidator),
        Some(config.clone()),
    ).await?;

    let app = Route::new()
        .nest("/", base_app)
        .nest("/export-api", api_service)
        .nest("/export-api/docs", swagger_ui)
        .at("/export-api/openapi.json", spec_endpoint);

    let client = TestClient::new(app);

    // 3. Get SwaggerUI content
    println!("\nTesting SwaggerUI endpoint:");
    let resp = client.get("/export-api/docs/").send().await;
    let status = resp.0.status();
    println!("  Status: {}", status);
    let swagger_ui_content = resp.0.into_body().into_bytes().await?;
    println!("  Response length: {} bytes", swagger_ui_content.len());
    assert_eq!(status.as_u16(), 200, "GET /export-api/docs/ failed");

    // 4. Get OpenAPI spec
    println!("\nTesting OpenAPI spec endpoint:");
    let resp = client.get("/export-api/openapi.json").send().await;
    let status = resp.0.status();
    println!("  Status: {}", status);
    let spec_content = resp.0.into_body().into_bytes().await?;
    println!("  Response length: {} bytes", spec_content.len());
    assert_eq!(status.as_u16(), 200, "GET /export-api/openapi.json failed");

    // 5. Create temporary directory for export
    let temp_dir = PathBuf::from("target/test_swagger_export");
    println!("\nCreating temporary directory: {:?}", temp_dir);
    fs::create_dir_all(&temp_dir).await?;

    // 6. Export files
    println!("Writing SwaggerUI and OpenAPI spec files");
    fs::write(temp_dir.join("swagger-ui.html"), swagger_ui_content).await?;
    fs::write(temp_dir.join("openapi.json"), spec_content).await?;

    // 7. Verify exported files
    println!("Verifying exported files");
    let ui_content = fs::read_to_string(temp_dir.join("swagger-ui.html")).await?;
    println!("  swagger-ui.html size: {} bytes", ui_content.len());
    println!("  Contains 'swagger-ui': {}", ui_content.contains("swagger-ui"));
    println!("  Contains 'Export Test API': {}", ui_content.contains("Export Test API"));
    assert!(ui_content.contains("swagger-ui"), "swagger-ui.html does not contain expected content");
    assert!(ui_content.contains("Export Test API"), "swagger-ui.html does not contain API title");

    let spec_content = fs::read_to_string(temp_dir.join("openapi.json")).await?;
    println!("  openapi.json size: {} bytes", spec_content.len());
    println!("  Contains 'openapi': {}", spec_content.contains("openapi"));
    println!("  Contains 'Export Test API': {}", spec_content.contains("Export Test API"));
    assert!(spec_content.contains("openapi"), "openapi.json does not contain expected content");
    assert!(spec_content.contains("Export Test API"), "openapi.json does not contain API title");

    // 8. Cleanup
    println!("Cleaning up temporary directory");
    fs::remove_dir_all(&temp_dir).await?;

    println!("=== Test completed successfully ===\n");
    Ok(())
}

#[OpenApi]
impl TestApi {
    /// Test endpoint
    #[oai(path = "/test", method = "get", tag = "ApiTags::Dynamic")]
    async fn test(&self) -> poem_openapi::payload::Json<String> {
        PoemJson("test".to_string())
    }

    /// Dynamic endpoint
    #[oai(path = "/dynamic/:path", method = "post", tag = "ApiTags::Dynamic")]
    async fn handle_dynamic(
        &self,
        path: Path<String>,
        payload: poem_openapi::payload::Json<DynamicRequest>
    ) -> poem_openapi::payload::Json<DynamicResponse> {
        PoemJson(DynamicResponse {
            result: format!("Processed {}: {}", path.0, payload.0.message),
        })
    }

    /// Register a new dynamic endpoint
    #[oai(path = "/register", method = "post", tag = "ApiTags::Dynamic")]
    async fn register_endpoint(
        &self,
        payload: poem_openapi::payload::Json<DynamicEndpoint>
    ) -> poem_openapi::payload::Json<DynamicResponse> {
        PoemJson(DynamicResponse {
            result: format!("Registered endpoint {} {}", payload.0.method, payload.0.path),
        })
    }

    /// Remove a dynamic endpoint
    #[oai(path = "/register/*path", method = "delete", tag = "ApiTags::Dynamic")]
    async fn remove_endpoint(
        &self,
        path: poem_openapi::param::Path<String>,
    ) -> poem_openapi::payload::Json<DynamicResponse> {
        PoemJson(DynamicResponse {
            result: format!("Removed endpoint {}", path.0),
        })
    }
}

fn create_default_oplog_response() -> GetOplogResponse {
    GetOplogResponse {
        entries: Vec::new(),
        next: None,
        first_index_in_chunk: 0,
        last_index: 0,
    }
}

// Add mock implementations for the required services
#[derive(Default)]
struct MockApiDefinitionRepo;

#[async_trait]
impl ApiDefinitionRepo for MockApiDefinitionRepo {
    async fn get(
        &self,
        _id: &str,
        _namespace: &str,
        _site_id: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        Ok(None)
    }

    async fn create(&self, _definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        Ok(())
    }

    async fn update(&self, _definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        Ok(())
    }

    async fn set_draft(
        &self,
        _id: &str,
        _namespace: &str,
        _site_id: &str,
        _is_draft: bool,
    ) -> Result<(), RepoError> {
        Ok(())
    }

    async fn get_draft(
        &self,
        _id: &str,
        _namespace: &str,
        _site_id: &str,
    ) -> Result<Option<bool>, RepoError> {
        Ok(None)
    }

    async fn get_all(&self, _site_id: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_all_versions(
        &self,
        _id: &str,
        _site_id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        Ok(vec![])
    }

    async fn delete(
        &self,
        _id: &str,
        _namespace: &str,
        _site_id: &str,
    ) -> Result<bool, RepoError> {
        Ok(true)
    }
}

#[derive(Default)]
struct MockApiDeploymentRepo;

#[async_trait]
impl ApiDeploymentRepo for MockApiDeploymentRepo {
    async fn get_by_id(
        &self,
        _id: &str,
        _site_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_by_id_and_version(
        &self,
        _id: &str,
        _site_id: &str,
        _version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_by_site(&self, _site_id: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_definitions_by_site(
        &self,
        _site_id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        Ok(vec![])
    }

    async fn create(&self, _deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        Ok(())
    }

    async fn delete(&self, _deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError> {
        Ok(true)
    }
}

#[derive(Default)]
struct MockSecuritySchemeService;

#[async_trait]
impl SecuritySchemeService<DefaultNamespace> for MockSecuritySchemeService {
    async fn get(
        &self,
        _id: &SecuritySchemeIdentifier,
        _namespace: &DefaultNamespace,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let redirect_url = RedirectUrl::new("http://localhost/callback".to_string())
            .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?;

        let all_signing_algs = vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256];
        
        let provider_metadata = CoreProviderMetadata::new(
            IssuerUrl::new("https://accounts.google.com".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            JsonWebKeySetUrl::new("https://www.googleapis.com/oauth2/v3/certs".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![CoreSubjectIdentifierType::Public],
            all_signing_algs.clone(),
            Default::default(),
        );

        Ok(SecuritySchemeWithProviderMetadata {
            security_scheme: SecurityScheme::new(
                Provider::Google,
                SecuritySchemeIdentifier::new("test-scheme".to_string()),
                ClientId::new("test-client".to_string()),
                ClientSecret::new("test-secret".to_string()),
                redirect_url,
                vec![Scope::new("read".to_string())],
            ),
            provider_metadata,
        })
    }

    async fn create(
        &self,
        _namespace: &DefaultNamespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let all_signing_algs = vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256];
        
        let provider_metadata = CoreProviderMetadata::new(
            IssuerUrl::new("https://accounts.google.com".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            JsonWebKeySetUrl::new("https://www.googleapis.com/oauth2/v3/certs".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![CoreSubjectIdentifierType::Public],
            all_signing_algs.clone(),
            Default::default(),
        );

        Ok(SecuritySchemeWithProviderMetadata {
            security_scheme: security_scheme.clone(),
            provider_metadata,
        })
    }
}

#[derive(Default)]
struct MockApiDefinitionValidator;

#[async_trait]
impl ApiDefinitionValidatorService<HttpApiDefinition> for MockApiDefinitionValidator {
    fn validate(
        &self,
        _definition: &HttpApiDefinition,
        _components: &[golem_service_base::model::Component],
    ) -> Result<(), ValidationErrors> {
        Ok(())
    }
} 