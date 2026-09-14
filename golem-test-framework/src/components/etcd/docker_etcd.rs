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
use std::fmt::{Debug, Formatter};
use std::time::{Duration, Instant};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use tracing::{error, info};

/// A single-node etcd server started in Docker.
///
/// The client port is exposed on a random host port; use [`client_url`](DockerEtcd::client_url)
/// to get the `http://127.0.0.1:<port>` URL for the service under test. TLS is not configured.
pub struct DockerEtcd {
    _container: ContainerHandle<GenericImage>,
    public_port: u16,
}

impl DockerEtcd {
    const CLIENT_PORT: u16 = 2379;
    const PEER_PORT: u16 = 2380;
    const DEFAULT_IMAGE_NAME: &'static str = "gcr.io/etcd-development/etcd";
    const DEFAULT_IMAGE_TAG: &'static str = "v3.5.17";

    pub async fn new() -> Self {
        Self::new_with_image(Self::DEFAULT_IMAGE_NAME, Self::DEFAULT_IMAGE_TAG).await
    }

    pub async fn new_with_image(image: &str, tag: &str) -> Self {
        info!("Starting etcd container ({image}:{tag})");

        let client_port = Self::CLIENT_PORT;
        let client_urls = format!("http://0.0.0.0:{client_port}");
        let peer_urls = format!("http://0.0.0.0:{}", Self::PEER_PORT);

        let container = tryhard::retry_fn(move || {
            // The official image's default entrypoint binds the client port to the container's
            // own localhost, so the mapped host port would refuse connections. Bind 0.0.0.0.
            let cmd = vec![
                "/usr/local/bin/etcd".to_string(),
                "--name".to_string(),
                "golem-test-etcd".to_string(),
                "--data-dir".to_string(),
                "/etcd-data".to_string(),
                "--listen-client-urls".to_string(),
                client_urls.clone(),
                "--advertise-client-urls".to_string(),
                client_urls.clone(),
                "--listen-peer-urls".to_string(),
                peer_urls.clone(),
                "--initial-advertise-peer-urls".to_string(),
                peer_urls.clone(),
                "--initial-cluster".to_string(),
                format!("golem-test-etcd={peer_urls}"),
                "--initial-cluster-token".to_string(),
                "golem-test-etcd".to_string(),
                "--initial-cluster-state".to_string(),
                "new".to_string(),
                "--log-level".to_string(),
                "info".to_string(),
            ];

            GenericImage::new(image, tag)
                .with_exposed_port(client_port.tcp())
                // etcd's zap logger writes to stderr.
                .with_wait_for(WaitFor::message_on_stderr("ready to serve client requests"))
                .with_cmd(cmd)
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start etcd container");

        let public_port = container
            .get_host_port_ipv4(client_port)
            .await
            .expect("Failed to get etcd host port");

        // The log line alone is not enough: the port forward can be established slightly after it.
        etcd_wait_for_startup("127.0.0.1", public_port, Duration::from_secs(60)).await;

        info!("etcd container started on port {public_port}");

        Self {
            _container: ContainerHandle::new(container),
            public_port,
        }
    }

    /// Returns the client URL, e.g. `http://127.0.0.1:2379`.
    pub fn client_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.public_port)
    }
}

impl Debug for DockerEtcd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerEtcd(port={})", self.public_port)
    }
}

async fn etcd_wait_for_startup(host: &str, port: u16, timeout: Duration) {
    info!(
        "Waiting for etcd client port on {host}:{port} (timeout {}s)",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        match tokio::net::TcpStream::connect(format!("{host}:{port}")).await {
            Ok(_) => {
                info!("etcd client port {port} is accepting connections");
                return;
            }
            Err(e) => {
                if start.elapsed() > timeout {
                    error!("etcd {host}:{port} did not become ready: {e}");
                    panic!("etcd {host}:{port} did not become ready within the timeout");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
