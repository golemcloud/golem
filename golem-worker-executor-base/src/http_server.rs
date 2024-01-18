use std::fmt::Display;
use std::net::SocketAddr;

use http::{Response, StatusCode};
use prometheus::{Encoder, Registry, TextEncoder};
use tokio::task::JoinHandle;
use tracing::{debug, info};
use warp::hyper::Body;
use warp::Filter;

/// The worker executor's HTTP interface provides Prometheus metrics and a healthcheck endpoint
pub struct HttpServerImpl {
    handle: JoinHandle<()>,
}

impl HttpServerImpl {
    pub fn new(
        addr: impl Into<SocketAddr> + Display + Send + 'static,
        registry: Registry,
    ) -> HttpServerImpl {
        let handle = tokio::spawn(server(addr, registry));
        HttpServerImpl { handle }
    }
}

impl Drop for HttpServerImpl {
    fn drop(&mut self) {
        info!("Stopping Http server...");
        self.handle.abort();
    }
}

async fn server(addr: impl Into<SocketAddr> + Display + Send, registry: Registry) {
    let healthcheck = warp::path!("healthcheck").map(|| {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("Worker executor is running"))
            .unwrap()
    });

    let metrics = warp::path!("metrics").map(move || prometheus_metrics(registry.clone()));

    info!("Http server started on {addr}");
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
