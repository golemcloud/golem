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
use std::time::Duration;
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::{Instrument, info};

pub async fn start_health_and_metrics_server(
    addr: impl ToSocketAddrs,
    registry: Registry,
    runtime_metrics_sampling_interval: Duration,
    body_message: &'static str,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<u16, anyhow::Error> {
    install_runtime_metrics(
        Handle::current(),
        registry.clone(),
        runtime_metrics_sampling_interval,
        join_set,
    );

    let app = Router::new()
        .route("/healthcheck", get(move || async move { body_message }))
        .route(
            "/metrics",
            get(move || async move { prometheus_metrics(registry.clone()) }),
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

pub fn prometheus_metrics(registry: Registry) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();

    let metric_families = registry.gather();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    Response::builder()
        .header("Content-Type", encoder.format_type())
        .body(Body::from(buffer))
        .unwrap()
}

fn install_runtime_metrics(
    runtime: Handle,
    registry: Registry,
    sampling_interval: Duration,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) {
    let recorder = match metrics_prometheus::Recorder::builder()
        .with_registry(registry)
        .try_build_and_install()
    {
        Ok(recorder) => recorder,
        Err(err) => {
            tracing::warn!(
                "Failed to install tokio runtime metrics recorder, runtime metrics will not be exported: {err}"
            );
            return;
        }
    };

    let reporter =
        tokio_metrics::RuntimeMetricsReporterBuilder::default().with_interval(sampling_interval);

    join_set.spawn_on(
        async move {
            reporter.describe_and_run().await;
            drop(recorder);
            Ok(())
        },
        &runtime,
    );
}
