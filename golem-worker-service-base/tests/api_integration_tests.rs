use test_r::test_gen;
use anyhow::Result;
use test_r::core::DynamicTestRegistration;

test_r::enable!();

#[cfg(test)]
mod api_integration_tests {
    use super::*;
    use axum::{
        routing::{get, post},
        Router, Json, response::IntoResponse,
        http::header,
    };
    use golem_worker_service_base::gateway_api_definition::http::{
        swagger_ui::{SwaggerUiConfig, generate_swagger_ui},
    };
    use serde::{Deserialize, Serialize};
    use std::{net::SocketAddr};
    use tokio::net::TcpListener;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use tower::ServiceBuilder;
    use tower_http::trace::TraceLayer;
    use http_body_util::{BodyExt, Empty};
    use utoipa::{OpenApi, ToSchema};
    use hyper::body::Bytes;

    // Types matching our OpenAPI spec
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    struct ComplexRequest {
        id: u32,
        name: String,
        flags: Vec<bool>,
        status: Status,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    #[serde(tag = "discriminator", content = "value")]
    enum Status {
        #[serde(rename = "Active")]
        Active,
        #[serde(rename = "Inactive")]
        Inactive { reason: String },
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    struct ApiResponse {
        success: bool,
        received: ComplexRequest,
    }

    #[derive(OpenApi)]
    #[openapi(
        paths(handle_complex_request),
        components(schemas(ComplexRequest, Status, ApiResponse))
    )]
    struct ApiDoc;

    #[utoipa::path(
        post,
        path = "/api/v1/complex",
        request_body = ComplexRequest,
        responses(
            (status = 200, description = "Success response", body = ApiResponse)
        )
    )]
    async fn handle_complex_request(
        Json(request): Json<ComplexRequest>,
    ) -> Json<ApiResponse> {
        // Echo back the request as success response
        Json(ApiResponse {
            success: true,
            received: request,
        })
    }

    async fn serve_openapi(
        axum::extract::Path((_api_id, _version)): axum::extract::Path<(String, String)>,
    ) -> Json<serde_json::Value> {
        let doc = ApiDoc::openapi();
        Json(serde_json::json!(doc))
    }

    async fn serve_swagger_ui() -> impl IntoResponse {
        let config = SwaggerUiConfig {
            enabled: true,
            path: "/docs".to_string(),
            title: Some("Test API".to_string()),
            theme: None,
            api_id: "test-api".to_string(),
            version: "1.0.0".to_string(),
        };

        let html = generate_swagger_ui(&config);
        
        (
            [(header::CONTENT_TYPE, "text/html")],
            html
        )
    }

    // Test server setup
    async fn setup_test_server() -> SocketAddr {
        // Create API routes
        let app = Router::new()
            .route("/api/v1/complex", post(handle_complex_request))
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

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_api_interaction(_test: &mut DynamicTestRegistration) -> Result<()> {
        // Start test server
        let addr = setup_test_server().await;
        let base_url = format!("http://{}", addr);
        
        let client = Client::builder(TokioExecutor::new())
            .build_http::<Empty<Bytes>>();

        // Test 1: Verify OpenAPI spec is served
        let spec_url = format!("{}/v1/api/definitions/test-api/version/1.0.0/export", base_url);
        let resp = client.get(spec_url.parse().unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let spec_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        
        // Verify OpenAPI spec content
        assert!(spec_json["paths"]["/api/v1/complex"]["post"]["requestBody"]["content"]["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap()
            .contains("ComplexRequest")
        );

        // Test 2: Verify Swagger UI is served
        let docs_url = format!("{}/docs", base_url);
        let resp = client.get(docs_url.parse().unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let docs_html = String::from_utf8(body.to_vec()).unwrap();
        assert!(docs_html.contains("swagger-ui"));

        // Test 3: Test actual API endpoint with reqwest (type-safe client)
        let client = reqwest::Client::new();
        
        // Success case
        let request = ComplexRequest {
            id: 42,
            name: "test".to_string(),
            flags: vec![true, false],
            status: Status::Active,
        };

        let resp = client.post(format!("{}/api/v1/complex", base_url))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        
        let result: ApiResponse = resp.json().await.unwrap();
        assert!(result.success);
        assert_eq!(result.received.id, 42);

        // Error case
        let request = ComplexRequest {
            id: 42,
            name: "test".to_string(),
            flags: vec![true, false],
            status: Status::Inactive { 
                reason: "testing error".to_string() 
            },
        };

        let resp = client.post(format!("{}/api/v1/complex", base_url))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        
        let result: ApiResponse = resp.json().await.unwrap();
        assert!(result.success);
        assert!(matches!(
            result.received.status,
            Status::Inactive { reason } if reason == "testing error"
        ));

        Ok(())
    }
} 