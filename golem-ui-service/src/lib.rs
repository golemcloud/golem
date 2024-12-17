#[cfg(test)]
test_r::enable!();

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
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
            // SPA fallback
            .fallback(get(Self::serve_index))
            .with_state(state)
            .layer(TraceLayer::new_for_http())
            .layer(cors);

        println!("UI Service listening on {}", self.addr);
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
