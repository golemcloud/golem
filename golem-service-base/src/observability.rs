// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use axum::Router;
use axum::body::Body;
use axum::response::IntoResponse;
use axum::routing::get;
use http::Response;
use prometheus::{Encoder, Registry, TextEncoder};
use std::sync::Arc;
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio::task::JoinSet;
use tracing::{Instrument, info};

/// A callback that renders additional metrics in Prometheus text exposition
/// format, appended to the output of the `prometheus`-crate registry on the
/// `/metrics` endpoint. Used to surface metrics from a second metrics façade
/// (e.g. the `metrics`-crate recorder driving tokio-metrics) on the same
/// scrape endpoint.
pub type ExtraMetrics = Arc<dyn Fn() -> String + Send + Sync>;

pub async fn start_health_and_metrics_server(
    addr: impl ToSocketAddrs,
    registry: Registry,
    body_message: &'static str,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<u16, anyhow::Error> {
    start_health_and_metrics_server_with_extra(addr, registry, None, body_message, join_set).await
}

pub async fn start_health_and_metrics_server_with_extra(
    addr: impl ToSocketAddrs,
    registry: Registry,
    extra: Option<ExtraMetrics>,
    body_message: &'static str,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<u16, anyhow::Error> {
    let app = Router::new()
        .route("/healthcheck", get(move || async move { body_message }))
        .route(
            "/metrics",
            get(move || {
                let extra = extra.clone();
                async move { prometheus_metrics(registry.clone(), extra) }
            }),
        );

    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    join_set.spawn(
        async move {
            axum::serve(listener, app).await?;
            Ok(())
        }
        .in_current_span(),
    );

    info!("Health and metrics server started on ports: http: {local_addr}");

    Ok(local_addr.port())
}

pub fn prometheus_metrics(registry: Registry, extra: Option<ExtraMetrics>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();

    let metric_families = registry.gather();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    if let Some(extra) = extra {
        buffer.extend_from_slice(extra().as_bytes());
    }

    Response::builder()
        .header("Content-Type", encoder.format_type())
        .body(Body::from(buffer))
        .unwrap()
}
