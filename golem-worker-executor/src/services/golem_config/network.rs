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

use golem_common::model::RetryConfig;
use golem_common::{SafeDisplay, grpc_uri};
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::net::IpAddr;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    /// The IP address the shard manager and the worker services use to reach this executor. When
    /// not set, the shard manager uses the source IP of the executor's requests, which is wrong
    /// where the network translates source addresses.
    #[serde(default)]
    pub advertised_ip: Option<IpAddr>,
    pub tls: GrpcServerTlsConfig,
    #[serde(default = "default_grpc_max_message_size")]
    pub max_message_size: usize,
}

fn default_grpc_max_message_size() -> usize {
    32 * 1024 * 1024
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);
        if let Some(advertised_ip) = &self.advertised_ip {
            let _ = writeln!(&mut result, "advertised IP: {advertised_ip}");
        }
        let _ = writeln!(&mut result, "max_message_size: {}", self.max_message_size);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9093,
            advertised_ip: None,
            tls: GrpcServerTlsConfig::disabled(),
            max_message_size: default_grpc_max_message_size(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl WorkerServiceGrpcConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

impl SafeDisplay for WorkerServiceGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for WorkerServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9007,
            client_config: GrpcClientConfig {
                retries_on_unavailable: RetryConfig::max_attempts_5(),
                connect_timeout: Duration::from_secs(10),
                ..Default::default()
            },
        }
    }
}

/// Access of agent code to `wasi:sockets` (TCP, UDP and IP name lookup, P2 and P3).
///
/// When `enabled` is `false`, every socket connect and bind fails with `access-denied` and every
/// name lookup fails with `permanent-resolver-failure`. The host-side clients that agents use
/// through `wasi:http`, `golem:websocket`, `golem:rdbms`, durable streams and MCP do not depend
/// on this setting.
///
/// When `enabled` is `true`, agent code can connect to any address that the executor can reach,
/// including loopback and private addresses of the deployment. The `network` permission of the
/// agent's cards matches host strings only and does not restrict address ranges.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentSocketsConfig {
    pub enabled: bool,
}

impl SafeDisplay for AgentSocketsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "enabled: {}", self.enabled);
        result
    }
}

/// Configuration for the HTTP connection pool used by outgoing wasi:http requests.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum HttpClientConfig {
    Enabled(HttpClientEnabledConfig),
    Disabled(HttpClientDisabledConfig),
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        HttpClientConfig::Enabled(HttpClientEnabledConfig::default())
    }
}

impl SafeDisplay for HttpClientConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            HttpClientConfig::Enabled(enabled) => {
                let _ = writeln!(&mut result, "enabled:");
                let _ = writeln!(&mut result, "{}", enabled.to_safe_string_indented());
            }
            HttpClientConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

/// Configuration for the shared HTTP connection pool.
///
/// A shared connection pool is created at executor startup and reused across
/// all workers, reducing TCP+TLS connection overhead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpClientEnabledConfig {
    /// Maximum number of idle connections per host.
    pub max_idle_per_host: usize,
    /// How long idle connections remain in the pool before being closed.
    #[serde(with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// Timeout for establishing new TCP connections via the pool.
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
    /// Maximum number of concurrent in-flight connections per host.
    pub max_connections_per_host: usize,
    /// Maximum total number of concurrent in-flight connections across all hosts.
    pub max_total_connections: usize,
    /// Maximum number of distinct host entries tracked in the per-host semaphore map.
    pub max_host_entries: usize,
}

impl Default for HttpClientEnabledConfig {
    fn default() -> Self {
        Self {
            max_idle_per_host: 8,
            idle_timeout: Duration::from_secs(90),
            connect_timeout: Duration::from_secs(30),
            max_connections_per_host: 20,
            max_total_connections: 200,
            max_host_entries: 1024,
        }
    }
}

impl SafeDisplay for HttpClientEnabledConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max idle per host: {}", self.max_idle_per_host);
        let _ = writeln!(
            &mut result,
            "idle timeout: {}s",
            self.idle_timeout.as_secs()
        );
        let _ = writeln!(
            &mut result,
            "connect timeout: {}s",
            self.connect_timeout.as_secs()
        );
        let _ = writeln!(
            &mut result,
            "max connections per host: {}",
            self.max_connections_per_host
        );
        let _ = writeln!(
            &mut result,
            "max total connections: {}",
            self.max_total_connections
        );
        let _ = writeln!(&mut result, "max host entries: {}", self.max_host_entries);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpClientDisabledConfig {}
