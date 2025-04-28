use axum::{routing::post, Router, response::IntoResponse};
use std::net::SocketAddr;
use embed_cohere::CohereClient;
use tower_http::services::ServeDir;

async fn run_server() {
    let app = Router::new()
        .route("/embed", post(handle_embed))
        .nest_service("/", ServeDir::new("public"));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn handle_embed(body: String) -> impl IntoResponse {
    let client = CohereClient::new().unwrap();
    match client.embed(vec![body], &Default::default()).await {
        Ok(embeddings) => axum::Json(embeddings),
        Err(e) => axum::Json(serde_json::json!({ "error": e.to_string() })),
    }
}

#[tokio::main]
async fn main() {
    run_server().await;
}