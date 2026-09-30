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
use crate::components::s3_mock::S3Mock;
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use testcontainers::GenericImage;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use tracing::info;

pub struct DockerS3Mock {
    container: ContainerHandle<GenericImage>,
    public_port: u16,
}

impl DockerS3Mock {
    const API_PORT: u16 = 9090;
    const DEFAULT_IMAGE_NAME: &'static str = "adobe/s3mock";
    const DEFAULT_IMAGE_TAG: &'static str = "5.2.3";
    const ACCESS_KEY_ID: &'static str = "test-access-key";
    const SECRET_ACCESS_KEY: &'static str = "test-secret-key";

    pub async fn new() -> Self {
        info!("Starting Adobe S3Mock container");

        let container = tryhard::retry_fn(|| {
            GenericImage::new(Self::DEFAULT_IMAGE_NAME, Self::DEFAULT_IMAGE_TAG)
                .with_exposed_port(Self::API_PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout(
                    "Started S3MockApplication.Companion",
                ))
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start Adobe S3Mock container");

        let public_port = container
            .get_host_port_ipv4(Self::API_PORT)
            .await
            .expect("Failed to get Adobe S3Mock host port");

        Self {
            container: ContainerHandle::new(container),
            public_port,
        }
    }
}

#[async_trait]
impl S3Mock for DockerS3Mock {
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

impl Debug for DockerS3Mock {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerS3Mock(port={})", self.public_port)
    }
}
