use crate::api::definition::BindingType;
use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use tower::{Service, ServiceExt};
use tower_http::services::ServeDir;
use std::path::PathBuf;
use poem::web::{Html, Path};
use poem::{handler, Response as PoemResponse, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUIBinding {
    pub spec_path: String,
    pub cors_allowed_origins: String,
}

impl SwaggerUIBinding {
    pub fn new(spec_path: String) -> Self {
        Self { spec_path }
    }

    pub fn create_handler(&self) -> impl Service<Request<Body>, Response = Response> {
        let spec_path = self.spec_path.clone();
        let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets/swagger-ui");

        let static_files = ServeDir::new(assets_dir);

        axum::Router::new()
            .fallback_service(static_files)
            .with_state(spec_path)
    }

    #[handler]
    pub async fn serve_ui(&self, _path: Path<String>) -> Result<PoemResponse> {
        let html = format!(
            r#"<!DOCTYPE html>
            <html>
              <head>
                <title>Swagger UI</title>
                <link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@4/swagger-ui.css">
              </head>
              <body>
                <div id="swagger-ui"></div>
                <script src="https://unpkg.com/swagger-ui-dist@4/swagger-ui-bundle.js"></script>
                <script>
                  window.onload = () => {{
                    window.ui = SwaggerUIBundle({{
                      url: '{}',
                      dom_id: '#swagger-ui',
                    }});
                  }};
                </script>
              </body>
            </html>"#,
            self.spec_path
        );
        Ok(Html(html).into())
    }

    fn generate_index_html(&self) -> String {
        format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <title>API Documentation</title>
    <link rel="stylesheet" type="text/css" href="./swagger-ui.css" />
    <link rel="icon" type="image/png" href="./favicon-32x32.png" sizes="32x32" />
    <link rel="icon" type="image/png" href="./favicon-16x16.png" sizes="16x16" />
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="./swagger-ui-bundle.js"></script>
    <script src="./swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = () => {{
            const ui = SwaggerUIBundle({{
                url: "{}",
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
            }});
        }}
    </script>
</body>
</html>"#,
            self.spec_path
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use hyper::body::to_bytes;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_swagger_ui_handler() {
        let binding = SwaggerUIBinding {
            spec_path: "/api/openapi/test".to_string(),
            cors_allowed_origins: "*".to_string(),
        };
        let handler = binding.create_handler();

        // Test root path
        let req = Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        
        let resp = handler.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        
        let body = to_bytes(resp.into_body()).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("/api/openapi/test"));
        assert!(html.contains("swagger-ui"));
    }

    #[tokio::test]
    async fn test_cors_headers() {
        let binding = SwaggerUIBinding {
            spec_path: "/api/openapi/test".to_string(),
            cors_allowed_origins: "*".to_string(),
        };
        let handler = binding.create_handler();

        let req = Request::builder()
            .uri("/")
            .method("OPTIONS")
            .body(Body::empty())
            .unwrap();
        
        let resp = handler.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        
        let headers = resp.headers();
        assert_eq!(
            headers.get("access-control-allow-origin").unwrap(),
            "*"
        );
        assert!(headers.get("access-control-allow-methods").is_some());
    }
}
