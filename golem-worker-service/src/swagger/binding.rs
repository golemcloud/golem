use crate::api::definition::BindingType;
use axum::{
    handler::Handler,
    http::{StatusCode, HeaderValue},
    body::Body,
    response::Response,
};
use serde::{Deserialize, Serialize};
use tower_http::{
    services::ServeDir,
    cors::CorsLayer,
};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUIBinding {
    pub spec_path: String,
    pub cors_allowed_origins: String,
}

impl BindingType for SwaggerUIBinding {
    fn create_handler(&self) -> Handler {
        let spec_path = self.spec_path.clone();
        let cors_allowed_origins = self.cors_allowed_origins.clone();
        let static_dir = PathBuf::from("swagger-ui");

        let cors_layer = CorsLayer::new()
            .allow_origin(
                cors_allowed_origins.split(",")
                    .map(|s| s.parse::<HeaderValue>().unwrap())
                    .collect::<Vec<_>>()
            )
            .allow_methods(vec!["GET", "OPTIONS"])
            .allow_headers(vec!["content-type"]);

        let static_handler = ServeDir::new(&static_dir)
            .with_cors(cors_layer);

        Handler::new(move |req| {
            let static_handler = static_handler.clone();
            let spec_path = spec_path.clone();

            async move {
                // Handle root path specially to inject correct spec_path
                if req.uri().path() == "/" {
                    let html = include_str!("../../assets/swagger-ui/index.html")
                        .replace("{{SPEC_URL}}", &spec_path);
                    
                    return Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/html")
                        .body(Body::from(html))
                        .unwrap());
                }

                // Serve static files
                match static_handler.serve(req).await {
                    Ok(response) => Ok(response),
                    Err(_) => Ok(Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap())
                }
            }
        })
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
