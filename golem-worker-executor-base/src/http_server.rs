// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Display;
use std::net::SocketAddr;

use http_02::{Response, StatusCode};
use prometheus::{Encoder, Registry, TextEncoder};
use tokio::task::JoinHandle;
use tracing::info;
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
        body_message: &'static str,
    ) -> HttpServerImpl {
        let handle = tokio::spawn(server(addr, registry, body_message));
        HttpServerImpl { handle }
    }
}

impl Drop for HttpServerImpl {
    fn drop(&mut self) {
        info!("Stopping Http server...");
        self.handle.abort();
    }
}

async fn server(
    addr: impl Into<SocketAddr> + Display + Send,
    registry: Registry,
    body_message: &'static str,
) {
    let healthcheck = warp::path!("healthcheck").map(move || {
        Response::builder()
            .status(StatusCode::OK)
            // .body(Body::from("Worker executor is running"))
            .body(Body::from(body_message))
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
