use poem::{
    get, handler, listener::TcpListener, middleware::Tracing, web::Json, web::Path, EndpointExt,
    Route, Server,
};

use std::num::ParseIntError;

#[handler]
fn echo(Path(input): Path<String>) -> String {
    common::echo(input)
}

#[handler]
fn calculate(Path(input): Path<u64>) -> Json<u64> {
    let _ = common::loop_fibonacci(50, input);
    Json(input)
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "poem=error");
    }

    let port: u16 = std::env::var("HTTP_PORT")
        .map_err(|e| e.to_string())
        .and_then(|v| v.parse().map_err(|e: ParseIntError| e.to_string()))
        .unwrap_or(3000);

    let addr = format!("0.0.0.0:{}", port);

    tracing_subscriber::fmt::init();

    let app = Route::new()
        .at("/echo/:input", get(echo))
        .at("/calculate/:input", get(calculate))
        .with(Tracing);

    Server::new(TcpListener::bind(addr))
        .name("rust-http-service")
        .run(app)
        .await
}
