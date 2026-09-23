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
use crate::components::minio::Minio;
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use tracing::info;

pub struct DockerMinio {
    container: ContainerHandle<GenericImage>,
    public_port: u16,
}

impl DockerMinio {
    const API_PORT: u16 = 9000;
    const DEFAULT_IMAGE_NAME: &'static str = "minio/minio";
    const DEFAULT_IMAGE_TAG: &'static str = "RELEASE.2025-01-20T14-49-07Z";
    const ACCESS_KEY_ID: &'static str = "minioadmin";
    const SECRET_ACCESS_KEY: &'static str = "minioadmin";

    pub async fn new() -> Self {
        info!("Starting MinIO container");

        let container = tryhard::retry_fn(|| {
            GenericImage::new(Self::DEFAULT_IMAGE_NAME, Self::DEFAULT_IMAGE_TAG)
                .with_exposed_port(Self::API_PORT.tcp())
                .with_wait_for(WaitFor::message_on_stderr("API:"))
                .with_env_var("MINIO_CONSOLE_ADDRESS", ":9001")
                .with_cmd(["server", "/data"])
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start MinIO container");

        let public_port = container
            .get_host_port_ipv4(Self::API_PORT)
            .await
            .expect("Failed to get MinIO host port");

        Self {
            container: ContainerHandle::new(container),
            public_port,
        }
    }
}

#[async_trait]
impl Minio for DockerMinio {
    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.public_port)
    }

    fn access_key_id(&self) -> &str {
        Self::ACCESS_KEY_ID
    }

    fn secret_access_key(&self) -> &str {
        Self::SECRET_ACCESS_KEY
    }

    async fn kill(&self) {
        self.container.kill().await
    }
}

impl Debug for DockerMinio {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerMinio(port={})", self.public_port)
    }
}
