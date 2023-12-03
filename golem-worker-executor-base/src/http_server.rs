use std::net::SocketAddr;

use http::{Response, StatusCode};
use prometheus::{Encoder, Registry, TextEncoder};
use tokio::task::JoinHandle;
use warp::hyper::Body;
use warp::Filter;

pub struct HttpServerImpl {
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

impl HttpServerImpl {
    pub fn new(addr: impl Into<SocketAddr> + Send + 'static, registry: Registry) -> HttpServerImpl {
        let handle = tokio::spawn(server(addr, registry));
        HttpServerImpl { handle }
    }
}

async fn server(addr: impl Into<SocketAddr> + Send, registry: Registry) {
    let healthcheck = warp::path!("healthcheck").map(|| {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("instance server is running"))
            .unwrap()
    });

    let metrics = warp::path!("metrics").map(move || prometheus_metrics(registry.clone()));

    warp::serve(healthcheck.or(metrics)).run(addr).await;
}

fn prometheus_metrics(registry: Registry) -> Response<Body> {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();

    let metric_families = registry.gather();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    Response::builder()
        .header("Content-Type", encoder.format_type())
        .body(Body::from(buffer))
        .unwrap()
}
