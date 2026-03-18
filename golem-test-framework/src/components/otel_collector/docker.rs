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

use crate::components::docker::ContainerHandle;
use crate::components::otel_collector::{wait_for_collector_startup, OtelCollector};
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::time::Duration;
use testcontainers::core::{IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::info;

pub struct DockerOtelCollector {
    container: ContainerHandle<GenericImage>,
    otlp_http_port: u16,
    output_dir: PathBuf,
}

impl DockerOtelCollector {
    const OTLP_HTTP_PORT: u16 = 4318;
    const HEALTH_PORT: u16 = 13133;

    const COLLECTOR_CONFIG: &'static str = r#"receivers:
  otlp:
    protocols:
      http:
        endpoint: "0.0.0.0:4318"

exporters:
  file/traces:
    path: /otel-output/otlp-traces.jsonl
    flush_interval: 1s
  file/logs:
    path: /otel-output/otlp-logs.jsonl
    flush_interval: 1s
  file/metrics:
    path: /otel-output/otlp-metrics.jsonl
    flush_interval: 1s

extensions:
  health_check:
    endpoint: "0.0.0.0:13133"

service:
  extensions: [health_check]
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [file/traces]
    logs:
      receivers: [otlp]
      exporters: [file/logs]
    metrics:
      receivers: [otlp]
      exporters: [file/metrics]
"#;

    pub async fn new() -> Self {
        info!("Starting OTel Collector container");

        let output_dir = tempfile::Builder::new()
            .prefix("otel-output-")
            .tempdir_in("/tmp")
            .expect("Failed to create temp dir for OTel output")
            .keep();

        for filename in ["otlp-traces.jsonl", "otlp-logs.jsonl", "otlp-metrics.jsonl"] {
            std::fs::File::create(output_dir.join(filename))
                .unwrap_or_else(|e| panic!("Failed to create {filename}: {e}"));
        }

        let config_dir = tempfile::Builder::new()
            .prefix("otel-config-")
            .tempdir_in("/tmp")
            .expect("Failed to create temp dir for collector config")
            .keep();
        let config_path = config_dir.join("config.yaml");
        std::fs::write(&config_path, Self::COLLECTOR_CONFIG)
            .expect("Failed to write collector config");

        let container: ContainerAsync<GenericImage> = tryhard::retry_fn(|| {
            GenericImage::new("otel/opentelemetry-collector-contrib", "0.120.0")
                .with_wait_for(WaitFor::message_on_stderr("Everything is ready."))
                .with_exposed_port(Self::OTLP_HTTP_PORT.tcp())
                .with_exposed_port(Self::HEALTH_PORT.tcp())
                .with_mount(Mount::bind_mount(
                    output_dir.to_str().unwrap(),
                    "/otel-output",
                ))
                .with_mount(Mount::bind_mount(
                    config_dir.to_str().unwrap(),
                    "/etc/otelcol",
                ))
                .with_cmd(["--config", "/etc/otelcol/config.yaml"])
                .with_startup_timeout(Duration::from_secs(120))
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start OTel Collector container");

        let otlp_http_port = container
            .get_host_port_ipv4(Self::OTLP_HTTP_PORT)
            .await
            .expect("Failed to get OTLP HTTP host port");

        let health_port = container
            .get_host_port_ipv4(Self::HEALTH_PORT)
            .await
            .expect("Failed to get health check host port");

        let health_url = format!("http://localhost:{health_port}");
        wait_for_collector_startup(&health_url, Duration::from_secs(30)).await;

        Self {
            container: ContainerHandle::new(container),
            otlp_http_port,
            output_dir,
        }
    }
}

#[async_trait]
impl OtelCollector for DockerOtelCollector {
    fn otlp_http_endpoint(&self) -> String {
        format!("http://localhost:{}", self.otlp_http_port)
    }

    fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

impl Debug for DockerOtelCollector {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerOtelCollector")
    }
}
