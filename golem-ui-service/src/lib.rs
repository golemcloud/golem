#[cfg(test)]
test_r::enable!();

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    routing::post,
    Router,
};
// use rib::
use clap::Parser;
use hyper::{client::HttpConnector, Client};
use mime_guess::from_path;
use rust_embed::RustEmbed;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::error;

// CLI Arguments
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct CliArgs {
    /// Server port
    #[arg(long, default_value = "3000")]
    pub port: u16,

    /// Server host
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// API base URL
    #[arg(long, default_value = "http://localhost:9881")]
    pub api_url: String,

    /// Development mode (uses CORS for Vite dev server)
    #[arg(long)]
    pub dev: bool,
}

// Embed the UI dist folder into the binary
#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct Assets;

#[derive(Clone)]
struct AppState {
    client: Client<HttpConnector>,
    api_url: String,
}

pub struct UiService {
    addr: SocketAddr,
    api_url: String,
    dev_mode: bool,
}

impl UiService {
    pub fn new(args: CliArgs) -> Self {
        let addr = SocketAddr::new(args.host.parse().expect("Invalid host address"), args.port);
        Self {
            addr,
            api_url: args.api_url,
            dev_mode: args.dev,
        }
    }

    async fn serve_index() -> impl IntoResponse {
        match Assets::get("index.html") {
            Some(content) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from(content.data))
                .unwrap()
                .into_response(),
            None => {
                error!("index.html not found in embedded assets");
                (StatusCode::NOT_FOUND, "Not found").into_response()
            }
        }
    }
    async fn validate_rib(body: Bytes) -> impl IntoResponse {
        // Parse the request body to get both rib and exports
        let request_body = match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(json) => json,
            Err(e) => {
                error!("Failed to parse request body: {}", e);
                return (StatusCode::BAD_REQUEST, "Failed to parse request body").into_response();
            }
        };

        // Extract rib text
        let rib_text = match request_body.get("rib") {
            Some(rib) => match rib.as_str() {
                Some(text) => text,
                None => {
                    error!("rib field must be a string");
                    return (StatusCode::BAD_REQUEST, "rib field must be a string").into_response();
                }
            },
            None => {
                error!("rib field not found in request body");
                return (
                    StatusCode::BAD_REQUEST,
                    "rib field not found in request body",
                )
                    .into_response();
            }
        };

        // Extract exports metadata
        let exports_metadata = match request_body.get("exports") {
            Some(exports) => match serde_json::from_value::<
                Vec<golem_wasm_ast::analysis::AnalysedExport>,
            >(exports.clone())
            {
                Ok(metadata) => metadata,
                Err(e) => {
                    error!("Failed to parse exports metadata: {}", e);
                    return (StatusCode::BAD_REQUEST, "Failed to parse exports metadata")
                        .into_response();
                }
            },
            None => {
                error!("exports field not found in request body");
                return (
                    StatusCode::BAD_REQUEST,
                    "exports field not found in request body",
                )
                    .into_response();
            }
        };

        // Parse the RIB expression
        let expr = match rib::Expr::from_text(rib_text) {
            Ok(expr) => expr,
            Err(e) => {
                error!("Failed to parse RIB expression: {}", e);
                return (StatusCode::BAD_REQUEST, e).into_response();
            }
        };

        // Define global variable type specs for request object
        let global_vars = vec![
            rib::GlobalVariableTypeSpec {
                variable_id: rib::VariableId::global("request".to_string()),
                path: rib::Path::from_elems(vec!["path"]),
                inferred_type: rib::InferredType::Str,
            },
            rib::GlobalVariableTypeSpec {
                variable_id: rib::VariableId::global("request".to_string()),
                path: rib::Path::from_elems(vec!["headers"]),
                inferred_type: rib::InferredType::Str,
            },
        ];

        // Validate RIB with restricted global variables
        match rib::compile_with_restricted_global_variables(
            &expr,
            &exports_metadata,
            Some(vec!["request".to_string()]),
            &global_vars,
        ) {
            Ok(result) => result,
            Err(e) => {
                error!("RIB validation failed: {}", e);
                return (
                    StatusCode::BAD_REQUEST,
                    format!("RIB validation failed: {}", e),
                )
                    .into_response();
            }
        };

        // Return success response
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("true"))
            .unwrap()
            .into_response()
    }
    async fn serve_static(uri: Uri) -> impl IntoResponse {
        let path = uri.path().trim_start_matches('/');

        // Special case for assets directory
        let asset_path = if path.starts_with("assets/") {
            path.to_string()
        } else {
            format!("assets/{}", path)
        };

        match Assets::get(&asset_path) {
            Some(content) => {
                let mime = from_path(&asset_path).first_or_octet_stream();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .header(header::CACHE_CONTROL, "public, max-age=3600")
                    .body(Body::from(content.data))
                    .unwrap()
                    .into_response()
            }
            None => {
                error!("Asset not found: {}", asset_path);
                (StatusCode::NOT_FOUND, "Not found").into_response()
            }
        }
    }

    async fn proxy_handler(
        State(state): State<Arc<AppState>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<Response<Body>, StatusCode> {
        // Reconstruct the URI for the backend API
        let path_and_query = uri.path_and_query().map(|x| x.as_str()).unwrap_or("");

        let backend_uri = format!(
            "{}{}",
            state.api_url,
            path_and_query
                .strip_prefix("/api")
                .unwrap_or(path_and_query)
        )
        .parse::<Uri>()
        .map_err(|e| {
            error!("Failed to parse backend URI: {}", e);
            StatusCode::BAD_REQUEST
        })?;

        // Create the proxied request
        let mut req = Request::builder()
            .uri(backend_uri.clone())
            .method(method.clone());

        // Forward relevant headers
        for (key, value) in headers.iter() {
            if key != "host" {
                req = req.header(key, value);
            }
        }

        let req = req.body(Body::from(body)).map_err(|e| {
            error!("Failed to create proxy request: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Send the request to the backend
        let response = state.client.request(req).await.map_err(|e| {
            error!("Proxy request failed: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

        Ok(response)
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Configure CORS based on mode
        let cors = if self.dev_mode {
            CorsLayer::new()
                .allow_origin(["http://localhost:5173".parse()?])
                .allow_methods(Any)
                .allow_headers(["content-type".parse()?, "authorization".parse()?])
        } else {
            CorsLayer::new()
                .allow_methods(Any)
                .allow_origin(["*".parse()?])
                .allow_headers(["content-type".parse()?, "authorization".parse()?])
        };

        let state = Arc::new(AppState {
            client: Client::new(),
            api_url: self.api_url.clone(),
        });

        let app = Router::new()
            // API proxy route
            .route(
                "/api/*path",
                get(Self::proxy_handler)
                    .post(Self::proxy_handler)
                    .put(Self::proxy_handler)
                    .delete(Self::proxy_handler)
                    .patch(Self::proxy_handler),
            )
            // Static assets route
            .route("/assets/*path", get(Self::serve_static))
            .route("/rib-validator", post(Self::validate_rib))
            // SPA fallback
            .fallback(get(Self::serve_index))
            .with_state(state)
            .layer(TraceLayer::new_for_http())
            .layer(cors);

        println!("UI Service listening on http://{}", self.addr);
        println!("API proxy configured for {}", self.api_url);
        if self.dev_mode {
            println!("Running in development mode");
        }
        axum::Server::bind(&self.addr)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;

        Ok(())
    }
}
