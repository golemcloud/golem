use poem::{
    get, handler, listener::TcpListener, middleware::Tracing, post, web::Json, web::Path,
    EndpointExt, Route, Server,
};

use serde::{Deserialize, Serialize};
use std::num::ParseIntError;

#[handler]
fn echo(Path(input): Path<String>) -> String {
    common::echo(input)
}

#[handler]
fn calculate(Path(input): Path<u64>) -> Json<u64> {
    let result = common::calculate_sum(10000, input).0;
    Json(result)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Data {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub timestamp: u64,
}

impl From<Data> for common::CommonData {
    fn from(data: Data) -> Self {
        common::CommonData {
            id: data.id,
            name: data.name,
            desc: data.desc,
            timestamp: data.timestamp,
        }
    }
}

impl From<common::CommonData> for Data {
    fn from(data: common::CommonData) -> Self {
        Data {
            id: data.id,
            name: data.name,
            desc: data.desc,
            timestamp: data.timestamp,
        }
    }
}

#[handler]
fn process(req: Json<Vec<Data>>) -> Json<Vec<Data>> {
    let input = req.0.into_iter().map(|i| i.into()).collect();
    let result = common::process_data(input);
    Json(result.into_iter().map(|i| i.into()).collect())
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
        .at("/process", post(process))
        .with(Tracing);

    Server::new(TcpListener::bind(addr))
        .name("rust-http-service")
        .run(app)
        .await
}
