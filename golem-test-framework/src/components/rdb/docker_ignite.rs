// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source Available License v1.0 (the "License");
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
use std::fmt::{Debug, Formatter};
use std::time::{Duration, Instant};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use tracing::{error, info};

/// A self-contained Apache Ignite 2.x thin-client node started in Docker.
///
/// The thin-client connector is exposed on a random host port; use
/// [`connection_url`](DockerIgniteRdb::connection_url) to get the
/// `ignite://127.0.0.1:<port>` URL for the service under test.
pub struct DockerIgniteRdb {
    _container: ContainerHandle<GenericImage>,
    public_port: u16,
}

impl DockerIgniteRdb {
    const DEFAULT_PORT: u16 = 10800;
    const DEFAULT_IMAGE_NAME: &'static str = "apacheignite/ignite";
    const DEFAULT_IMAGE_TAG: &'static str = "2.17.0";

    pub async fn new() -> Self {
        Self::new_with_image(Self::DEFAULT_IMAGE_NAME, Self::DEFAULT_IMAGE_TAG).await
    }

    pub async fn new_with_image(image: &str, tag: &str) -> Self {
        info!("Starting Apache Ignite container ({image}:{tag})");

        let port = Self::DEFAULT_PORT;

        let container = tryhard::retry_fn(move || {
            GenericImage::new(image, tag)
                .with_exposed_port(port.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Ignite node started OK"))
                // Allow DML inside transactions over TRANSACTIONAL-atomicity caches.
                .with_env_var("JVM_OPTS", "-DIGNITE_ALLOW_DML_INSIDE_TRANSACTION=true")
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start Apache Ignite container");

        let public_port = container
            .get_host_port_ipv4(port)
            .await
            .expect("Failed to get Ignite host port");

        ignite_wait_for_startup("127.0.0.1", public_port, Duration::from_secs(60)).await;

        info!("Apache Ignite container started on port {public_port}");

        Self {
            _container: ContainerHandle::new(container),
            public_port,
        }
    }

    /// Returns the thin-client connection URL, e.g. `ignite://127.0.0.1:10800`.
    pub fn connection_url(&self) -> String {
        format!("ignite://127.0.0.1:{}", self.public_port)
    }
}

impl Debug for DockerIgniteRdb {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerIgniteRdb(port={})", self.public_port)
    }
}

async fn ignite_wait_for_startup(host: &str, port: u16, timeout: Duration) {
    info!(
        "Waiting for Ignite thin-client on {host}:{port} (timeout {}s)",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        match tokio::net::TcpStream::connect(format!("{host}:{port}")).await {
            Ok(_) => {
                info!("Ignite thin-client port {port} is accepting connections");
                return;
            }
            Err(e) => {
                if start.elapsed() > timeout {
                    error!("Ignite {host}:{port} did not become ready: {e}");
                    panic!("Ignite {host}:{port} did not become ready within the timeout");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
