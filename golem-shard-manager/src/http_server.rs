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
            .body(Body::from("shard manager is running"))
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
