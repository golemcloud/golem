#[cfg(test)]
mod api_integration_tests {
    use axum::{
        routing::{get, post},
        Router, response::IntoResponse,
        http::header,
        Json as AxumJson,
    };
    use golem_worker_service_base::gateway_api_definition::http::{
        swagger_ui::{SwaggerUiConfig, create_swagger_ui},
    };
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tower::ServiceBuilder;
    use tower_http::trace::TraceLayer;
    use poem_openapi::{OpenApi, Object, payload::Json};
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::client::legacy::Client;
    use hyper::body::Bytes;
    use http_body_util::{Empty, BodyExt};
    use reqwest;

    // Types matching our OpenAPI spec
    #[derive(Debug, Serialize, Deserialize, Object)]
    struct ComplexRequest {
        id: u32,
        name: String,
        flags: Vec<bool>,
        status: Status,
    }

    #[derive(Debug, Serialize, Deserialize, Object)]
    #[oai(skip_serializing_if_is_none)]
    struct Status {
        #[oai(rename = "type")]
        status_type: String,
        reason: Option<String>,
    }

    #[derive(Debug, Serialize, Deserialize, Object)]
    struct CustomApiResponse {
        success: bool,
        received: ComplexRequest,
    }

    #[derive(Clone)]
    struct ApiDoc;

    #[OpenApi]
    impl ApiDoc {
        /// Handle a complex request
        #[oai(path = "/api/v1/complex", method = "post")]
        async fn handle_complex_request(
            &self,
            request: Json<ComplexRequest>,
        ) -> Json<CustomApiResponse> {
            // Echo back the request as success response
            Json(CustomApiResponse {
                success: true,
                received: request.0,
            })
        }
    }

    async fn serve_openapi(
        axum::extract::Path((_api_id, _version)): axum::extract::Path<(String, String)>,
    ) -> AxumJson<serde_json::Value> {
        let service = create_swagger_ui(ApiDoc, &SwaggerUiConfig {
            enabled: true,
            title: Some("Test API".to_string()),
            version: Some("1.0".to_string()),
            server_url: None,
        });
        let spec = service.spec();
        AxumJson(serde_json::from_str(&spec).unwrap())
    }

    async fn serve_swagger_ui() -> impl IntoResponse {
        let config = SwaggerUiConfig {
            enabled: true,
            title: Some("Test API".to_string()),
            version: Some("1.0".to_string()),
            server_url: None,
        };

        let service = create_swagger_ui(ApiDoc, &config);
        let html = service.swagger_ui_html();
        
        (
            [(header::CONTENT_TYPE, "text/html")],
            html
        )
    }

    async fn handle_complex_request_axum(
        AxumJson(request): AxumJson<ComplexRequest>,
    ) -> AxumJson<CustomApiResponse> {
        AxumJson(CustomApiResponse {
            success: true,
            received: request,
        })
    }

    // Test server setup
    async fn setup_test_server() -> SocketAddr {
        // Create API routes
        let app = Router::new()
            .route("/api/v1/complex", post(handle_complex_request_axum))
            .route("/v1/api/definitions/:api_id/version/:version/export", get(serve_openapi))
            .route("/docs", get(serve_swagger_ui))
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()));

        // Find available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Start server
        tokio::spawn(async move {
            let server = axum::serve(listener, app);
            server.await.unwrap();
        });

        addr
    }

    #[tokio::test]
    async fn test_api_interaction() -> Result<(), Box<dyn std::error::Error>> {
        // Start test server
        let addr = setup_test_server().await;
        let base_url = format!("http://{}", addr);
        
        let client = Client::builder(hyper_util::rt::TokioExecutor::new())
            .build::<_, Empty<Bytes>>(HttpConnector::new());

        // Test 1: Verify OpenAPI spec is served
        let spec_url = format!("{}/v1/api/definitions/test-api/version/1.0.0/export", base_url);
        let resp = client.get(spec_url.parse().unwrap()).await?;
        assert_eq!(resp.status(), 200);
        
        let body = resp.into_body().collect().await?.to_bytes();
        let spec_json: serde_json::Value = serde_json::from_slice(&body)?;
        
        // Write OpenAPI spec to files for debugging
        let target_dir = std::path::Path::new("target");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir)?;
        }
        
        let json_path = target_dir.join("openapi-spec.json");
        let yaml_path = target_dir.join("openapi-spec.yaml");
        
        std::fs::write(
            &json_path,
            serde_json::to_string_pretty(&spec_json)?
        )?;
        std::fs::write(
            &yaml_path,
            serde_yaml::to_string(&spec_json)?
        )?;
        
        // Print the spec for debugging
        println!("OpenAPI Spec: {}", serde_json::to_string_pretty(&spec_json)?);
        
        // Verify OpenAPI spec content with more detailed error handling
        let paths = spec_json.get("paths").expect("OpenAPI spec should have paths");
        let complex_path = paths.get("/api/v1/complex").expect("Should have /api/v1/complex path");
        let post_method = complex_path.get("post").expect("Should have POST method");
        let request_body = post_method.get("requestBody").expect("Should have requestBody");
        let content = request_body.get("content").expect("Should have content");
        let json_content = content.get("application/json; charset=utf-8").expect("Should have application/json content");
        let schema = json_content.get("schema").expect("Should have schema");
        let schema_ref = schema.get("$ref").expect("Should have $ref");
        
        assert!(schema_ref.as_str().unwrap().contains("ComplexRequest"), 
            "Schema ref should reference ComplexRequest, got: {}", schema_ref);

        // Test 2: Verify Swagger UI is served
        let docs_url = format!("{}/docs", base_url);
        let resp = client.get(docs_url.parse().unwrap()).await?;
        assert_eq!(resp.status(), 200);
        
        let body = resp.into_body().collect().await?.to_bytes();
        let docs_html = String::from_utf8(body.to_vec())?;
        assert!(docs_html.contains("swagger-ui"));

        // Test 3: Test actual API endpoint with reqwest (type-safe client)
        let client = reqwest::Client::new();
        
        // Success case
        let request = ComplexRequest {
            id: 42,
            name: "test".to_string(),
            flags: vec![true, false],
            status: Status {
                status_type: "Active".to_string(),
                reason: None,
            },
        };

        let resp = client.post(format!("{}/api/v1/complex", base_url))
            .json(&request)
            .send()
            .await?;
        assert_eq!(resp.status(), 200);
        
        let result: CustomApiResponse = resp.json().await?;
        assert!(result.success);
        assert_eq!(result.received.id, 42);

        // Error case
        let request = ComplexRequest {
            id: 42,
            name: "test".to_string(),
            flags: vec![true, false],
            status: Status {
                status_type: "Inactive".to_string(),
                reason: Some("testing error".to_string()),
            },
        };

        let resp = client.post(format!("{}/api/v1/complex", base_url))
            .json(&request)
            .send()
            .await?;
        assert_eq!(resp.status(), 200);
        
        let result: CustomApiResponse = resp.json().await?;
        assert!(result.success);
        assert!(matches!(
            result.received.status.reason,
            Some(reason) if reason == "testing error"
        ));

        Ok(())
    }
} 