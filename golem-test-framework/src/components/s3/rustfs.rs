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
use crate::components::s3::S3Server;
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::{Duration, Instant};
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use tracing::info;

/// A single-node RustFS server in a container.
pub struct DockerRustFs {
    container: ContainerHandle<GenericImage>,
    public_port: u16,
}

impl DockerRustFs {
    const API_PORT: u16 = 9000;
    const IMAGE_NAME: &'static str = "rustfs/rustfs";
    /// The tag `1.0.1`, pinned by the digest of its multi-platform index.
    const IMAGE_TAG: &'static str =
        "1.0.1@sha256:1803faef57627e2d9c2e7d89d655d712ddded5389040054987163043fecb6a3c";
    const ACCESS_KEY_ID: &'static str = "test-access-key";
    const SECRET_ACCESS_KEY: &'static str = "test-secret-key";
    /// The RustFS entrypoint prints this line when it starts the server process.
    const STARTED_MESSAGE: &'static str = "Starting: /usr/bin/rustfs";
    /// The S3 port answers this path with 200 once storage, IAM and locks are ready. `/health`
    /// answers 200 earlier, while S3 requests still get 503 ("Service not ready: waiting for
    /// iam").
    const READY_PATH: &'static str = "/health/ready";
    const READY_TIMEOUT: Duration = Duration::from_secs(60);
    const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);

    pub async fn new() -> Self {
        info!("Starting RustFS container");
        let started_at = Instant::now();

        let container = tryhard::retry_fn(|| {
            GenericImage::new(Self::IMAGE_NAME, Self::IMAGE_TAG)
                .with_exposed_port(Self::API_PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout(Self::STARTED_MESSAGE))
                .with_env_var("RUSTFS_ACCESS_KEY", Self::ACCESS_KEY_ID)
                .with_env_var("RUSTFS_SECRET_KEY", Self::SECRET_ACCESS_KEY)
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start RustFS container");

        let public_port = container
            .get_host_port_ipv4(Self::API_PORT)
            .await
            .expect("Failed to get RustFS host port");

        let server = Self {
            container: ContainerHandle::new(container),
            public_port,
        };
        server.wait_until_ready().await;
        info!(
            "RustFS container is ready after {} ms",
            started_at.elapsed().as_millis()
        );
        server
    }

    async fn wait_until_ready(&self) {
        let url = format!("{}{}", self.endpoint(), Self::READY_PATH);
        let client = reqwest::Client::new();
        let deadline = Instant::now() + Self::READY_TIMEOUT;
        loop {
            let status = client
                .get(&url)
                .send()
                .await
                .map(|response| response.status());
            if matches!(&status, Ok(status) if status.is_success()) {
                return;
            }
            if Instant::now() >= deadline {
                panic!("RustFS did not answer {url} with a success in time: {status:?}");
            }
            tokio::time::sleep(Self::READY_POLL_INTERVAL).await;
        }
    }
}

#[async_trait]
impl S3Server for DockerRustFs {
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

impl Debug for DockerRustFs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerRustFs(port={})", self.public_port)
    }
}
