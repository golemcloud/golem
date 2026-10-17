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
/// By default the client port is plaintext and exposed on a random host port; use
/// [`client_url`](DockerEtcd::client_url) to get the URL for the service under test.
/// [`DockerEtcdOptions`] adds TLS, client certificate checks, a fixed host port and extra flags.
pub struct DockerEtcd {
    container: ContainerHandle<GenericImage>,
    public_port: u16,
    tls: bool,
}

/// What a [`DockerEtcd`] needs beyond a plain single node.
#[derive(Clone, Default)]
pub struct DockerEtcdOptions {
    server_tls: Option<(String, String)>,
    client_ca_pem: Option<String>,
    host_port: Option<u16>,
    extra_flags: Vec<String>,
}

impl DockerEtcdOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// TLS on the client port, with this server certificate and key (PEM). The client URL is then
    /// `https://localhost:<port>`, so the certificate must name `localhost`.
    pub fn with_tls(
        mut self,
        server_cert_pem: impl Into<String>,
        server_key_pem: impl Into<String>,
    ) -> Self {
        self.server_tls = Some((server_cert_pem.into(), server_key_pem.into()));
        self
    }

    /// Requires a client certificate signed by this CA (PEM); needs [`Self::with_tls`].
    pub fn with_client_cert_auth(mut self, client_ca_pem: impl Into<String>) -> Self {
        self.client_ca_pem = Some(client_ca_pem.into());
        self
    }

    /// Publishes the client port on this host port instead of a random one, so that it stays the
    /// same across [`DockerEtcd::restart`].
    pub fn with_host_port(mut self, host_port: u16) -> Self {
        self.host_port = Some(host_port);
        self
    }

    /// One more etcd command-line flag, such as `--auth-token-ttl=2`.
    pub fn with_flag(mut self, flag: impl Into<String>) -> Self {
        self.extra_flags.push(flag.into());
        self
    }
}

impl DockerEtcd {
    const CLIENT_PORT: u16 = 2379;
    const PEER_PORT: u16 = 2380;
    const DEFAULT_IMAGE_NAME: &'static str = "gcr.io/etcd-development/etcd";
    const DEFAULT_IMAGE_TAG: &'static str = "v3.5.17";
    const SERVER_CERT: &'static str = "/certs/server.pem";
    const SERVER_KEY: &'static str = "/certs/server-key.pem";
    const CLIENT_CA: &'static str = "/certs/client-ca.pem";

    pub async fn new() -> Self {
        Self::new_with_image(Self::DEFAULT_IMAGE_NAME, Self::DEFAULT_IMAGE_TAG).await
    }

    pub async fn new_with_image(image: &str, tag: &str) -> Self {
        Self::start(image, tag, DockerEtcdOptions::default()).await
    }

    pub async fn new_with_options(options: DockerEtcdOptions) -> Self {
        Self::start(Self::DEFAULT_IMAGE_NAME, Self::DEFAULT_IMAGE_TAG, options).await
    }

    async fn start(image: &str, tag: &str, options: DockerEtcdOptions) -> Self {
        info!("Starting etcd container ({image}:{tag})");

        let client_port = Self::CLIENT_PORT;
        let tls = options.server_tls.is_some();
        let scheme = if tls { "https" } else { "http" };
        let client_urls = format!("{scheme}://0.0.0.0:{client_port}");
        let peer_urls = format!("http://0.0.0.0:{}", Self::PEER_PORT);

        let container = tryhard::retry_fn(move || {
            // The official image's default entrypoint binds the client port to the container's
            // own localhost, so the mapped host port would refuse connections. Bind 0.0.0.0.
            let mut cmd = vec![
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
            if options.server_tls.is_some() {
                cmd.push(format!("--cert-file={}", Self::SERVER_CERT));
                cmd.push(format!("--key-file={}", Self::SERVER_KEY));
            }
            if options.client_ca_pem.is_some() {
                cmd.push(format!("--trusted-ca-file={}", Self::CLIENT_CA));
                cmd.push("--client-cert-auth".to_string());
            }
            cmd.extend(options.extra_flags.iter().cloned());

            let mut request = GenericImage::new(image, tag)
                .with_exposed_port(client_port.tcp())
                // etcd's zap logger writes to stderr.
                .with_wait_for(WaitFor::message_on_stderr("ready to serve client requests"))
                .with_cmd(cmd);
            if let Some((cert, key)) = &options.server_tls {
                request = request
                    .with_copy_to(Self::SERVER_CERT, cert.clone().into_bytes())
                    .with_copy_to(Self::SERVER_KEY, key.clone().into_bytes());
            }
            if let Some(ca) = &options.client_ca_pem {
                request = request.with_copy_to(Self::CLIENT_CA, ca.clone().into_bytes());
            }
            if let Some(host_port) = options.host_port {
                request = request.with_mapped_port(host_port, client_port.tcp());
            }
            request.start()
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
            container: ContainerHandle::new(container),
            public_port,
            tls,
        }
    }

    /// Returns the client URL: `http://127.0.0.1:<port>`, or `https://localhost:<port>` with TLS.
    pub fn client_url(&self) -> String {
        if self.tls {
            format!("https://localhost:{}", self.public_port)
        } else {
            format!("http://127.0.0.1:{}", self.public_port)
        }
    }

    /// Stops etcd and starts it again with its data; open client connections break. Use a fixed
    /// host port ([`DockerEtcdOptions::with_host_port`]), or the port can change.
    pub async fn restart(&self) {
        self.container.restart().await;
        etcd_wait_for_startup("127.0.0.1", self.public_port, Duration::from_secs(60)).await;
    }

    /// Stops etcd, keeping its data, until [`Self::start_again`].
    pub async fn stop(&self) {
        self.container.stop().await;
    }

    pub async fn start_again(&self) {
        self.container.start().await;
        etcd_wait_for_startup("127.0.0.1", self.public_port, Duration::from_secs(60)).await;
    }
}

impl Debug for DockerEtcd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DockerEtcd(port={}, tls={})", self.public_port, self.tls)
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
