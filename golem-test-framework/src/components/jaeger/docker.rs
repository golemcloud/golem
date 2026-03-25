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
use crate::components::jaeger::{wait_for_startup, Jaeger};
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::info;

pub struct DockerJaeger {
    container: ContainerHandle<GenericImage>,
    otlp_http_port: u16,
    query_port: u16,
}

impl DockerJaeger {
    const OTLP_HTTP_PORT: u16 = 4318;
    const QUERY_PORT: u16 = 16686;

    pub async fn new() -> Self {
        info!("Starting Jaeger container");

        let container: ContainerAsync<GenericImage> = tryhard::retry_fn(|| {
            GenericImage::new("jaegertracing/all-in-one", "1.76.0")
                .with_wait_for(WaitFor::seconds(5))
                .with_exposed_port(Self::OTLP_HTTP_PORT.tcp())
                .with_exposed_port(Self::QUERY_PORT.tcp())
                .with_env_var("COLLECTOR_OTLP_ENABLED", "true")
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start Jaeger container");

        let otlp_http_port = container
            .get_host_port_ipv4(Self::OTLP_HTTP_PORT)
            .await
            .expect("Failed to get OTLP HTTP host port");

        let query_port = container
            .get_host_port_ipv4(Self::QUERY_PORT)
            .await
            .expect("Failed to get Jaeger query host port");

        let query_url = format!("http://localhost:{query_port}");
        wait_for_startup(&query_url, Duration::from_secs(30)).await;

        Self {
            container: ContainerHandle::new(container),
            otlp_http_port,
            query_port,
        }
    }
}

#[async_trait]
impl Jaeger for DockerJaeger {
    fn otlp_http_endpoint(&self) -> String {
        format!("http://localhost:{}", self.otlp_http_port)
    }

    fn query_url(&self) -> String {
        format!("http://localhost:{}", self.query_port)
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

impl Debug for DockerJaeger {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerJaeger")
    }
}
