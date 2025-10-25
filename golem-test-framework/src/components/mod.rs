// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_api_grpc::proto::grpc::health::v1::health_check_response::ServingStatus;
use golem_api_grpc::proto::grpc::health::v1::HealthCheckRequest;
use golem_client::api::HealthCheckClient;
use golem_client::Security;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::str::FromStr;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info, trace};
use tracing::{error, warn, Level};
use url::Url;

pub mod cloud_service;
pub mod component_compilation_service;
pub mod component_service;
mod docker;
pub mod rdb;
pub mod redis;
pub mod redis_monitor;
pub mod service;
pub mod shard_manager;
pub mod worker_executor;
pub mod worker_executor_cluster;
pub mod worker_service;

pub struct ChildProcessLogger {
    _out_handle: JoinHandle<()>,
    _err_handle: JoinHandle<()>,
}

impl ChildProcessLogger {
    pub fn log_child_process(
        prefix: &str,
        out_level: Level,
        err_level: Level,
        child: &mut Child,
    ) -> Self {
        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stdout"));

        let stderr = child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stderr"));

        let prefix_clone = prefix.to_string();
        let stdout_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match out_level {
                    Level::TRACE => trace!("{} {}", prefix_clone, line.unwrap()),
                    Level::DEBUG => debug!("{} {}", prefix_clone, line.unwrap()),
                    Level::INFO => info!("{} {}", prefix_clone, line.unwrap()),
                    Level::WARN => warn!("{} {}", prefix_clone, line.unwrap()),
                    Level::ERROR => error!("{} {}", prefix_clone, line.unwrap()),
                }
            }
        });

        let prefix_clone = prefix.to_string();
        let stderr_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match err_level {
                    Level::TRACE => trace!("{} {}", prefix_clone, line.unwrap()),
                    Level::DEBUG => debug!("{} {}", prefix_clone, line.unwrap()),
                    Level::INFO => info!("{} {}", prefix_clone, line.unwrap()),
                    Level::WARN => warn!("{} {}", prefix_clone, line.unwrap()),
                    Level::ERROR => error!("{} {}", prefix_clone, line.unwrap()),
                }
            }
        });

        Self {
            _out_handle: stdout_handle,
            _err_handle: stderr_handle,
        }
    }
}

pub async fn wait_for_startup_grpc(host: &str, grpc_port: u16, name: &str, timeout: Duration) {
    info!(
        "Waiting for {name} (GRPC) start on host {host}:{grpc_port}, timeout: {}s",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let success =
            match golem_api_grpc::proto::grpc::health::v1::health_client::HealthClient::connect(
                format!("http://{host}:{grpc_port}"),
            )
            .await
            {
                Ok(mut client) => match client
                    .check(HealthCheckRequest {
                        service: "".to_string(),
                    })
                    .await
                {
                    Ok(response) => response.into_inner().status == ServingStatus::Serving as i32,
                    Err(err) => {
                        debug!("Health request for {name} returned with an error: {err:?}");
                        false
                    }
                },
                Err(_) => false,
            };
        if success {
            break;
        } else {
            if start.elapsed() > timeout {
                panic!("Failed to verify that {name} is running");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

pub async fn wait_for_startup_http(host: &str, http_port: u16, name: &str, timeout: Duration) {
    info!(
        "Waiting for {name} (HTTP) start on host {host}:{http_port}, timeout: {}s",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let client = golem_client::api::HealthCheckClientLive {
            context: golem_client::Context {
                client: new_reqwest_client(),
                base_url: Url::from_str(&format!("http://{host}:{http_port}"))
                    .expect("Can't parse HTTP URL for health check"),
                security_token: Security::Empty,
            },
        };

        let success = match client.healthcheck().await {
            Ok(_) => true,
            Err(err) => {
                debug!("Health request for {name} returned with an error: {err:?}");
                false
            }
        };
        if success {
            break;
        } else {
            if start.elapsed() > timeout {
                panic!("Failed to verify that {name} is running");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

struct EnvVarBuilder {
    env_vars: HashMap<String, String>,
}

impl EnvVarBuilder {
    fn default() -> Self {
        Self {
            env_vars: HashMap::new(),
        }
    }

    fn golem_service(verbosity: Level) -> Self {
        Self::default()
            .with_rust_log_with_dep_defaults(verbosity)
            .with_rust_back_log()
            .with_tracing_from_env()
    }

    fn with(mut self, name: &str, value: String) -> Self {
        self.env_vars.insert(name.to_string(), value);
        self
    }

    fn with_str(self, name: &str, value: &str) -> Self {
        self.with(name, value.to_string())
    }

    fn with_all(mut self, env_vars: HashMap<String, String>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }

    fn with_rust_log_with_dep_defaults(self, verbosity: Level) -> Self {
        let rust_log_level_str = verbosity.as_str().to_lowercase();
        self.with(
            "RUST_LOG",
            format!(
                "{rust_log_level_str},\
                cranelift_codegen=warn,\
                wasmtime_cranelift=warn,\
                wasmtime_jit=warn,\
                h2=warn,\
                hyper=warn,\
                tower=warn,\
                fred=error"
            ),
        )
    }

    fn with_rust_back_log(self) -> Self {
        self.with_str("RUST_BACKLOG", "1")
    }

    fn with_tracing_from_env(mut self) -> Self {
        for (name, value) in
            std::env::vars().filter(|(name, _value)| name.starts_with("GOLEM__TRACING_"))
        {
            self.env_vars.insert(name, value);
        }
        self
    }

    fn build(self) -> HashMap<String, String> {
        self.env_vars
    }
}

fn new_reqwest_client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client")
}
