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


use http_02::{Response, StatusCode};
use prometheus::{Encoder, Registry, TextEncoder};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tracing::{info, Instrument};
use warp::hyper::Body;
use warp::Filter;

pub async fn start_health_and_metrics_server(
    addr: impl ToSocketAddrs,
    registry: Registry,
    body_message: &'static str,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<u16, anyhow::Error> {
    let healthcheck = warp::path!("healthcheck").map(move || {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(body_message))
            .unwrap()
    });

    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    let metrics = warp::path!("metrics").map(move || prometheus_metrics(registry.clone()));

    join_set.spawn(
        async move {
            warp::serve(healthcheck.or(metrics))
                .run_incoming(TcpListenerStream::new(listener))
                .await;
            Ok(())
        }
        .in_current_span(),
    );

    info!("Http server started on {local_addr}");

    Ok(local_addr.port())
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
