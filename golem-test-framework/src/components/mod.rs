// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_api_grpc::proto::grpc::health::v1::health_check_response::ServingStatus;
use golem_api_grpc::proto::grpc::health::v1::HealthCheckRequest;
use once_cell::sync::Lazy;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info, trace};
use tracing::{error, warn, Level};

pub mod k8s;
pub mod rdb;
pub mod redis;
pub mod redis_monitor;
pub mod shard_manager;
pub mod component_service;
pub mod worker_executor;
pub mod worker_executor_cluster;
pub mod worker_service;

const NETWORK: &str = "golem_test_network";

struct ChildProcessLogger {
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

async fn wait_for_startup_grpc(host: &str, grpc_port: u16, name: &str) {
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
            if start.elapsed().as_secs() > 90 {
                panic!("Failed to verify that {name} is running");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

// Using a global docker client to avoid the restrictions of the testcontainers library,
// binding the container lifetime to the client.
static DOCKER: Lazy<testcontainers::clients::Cli> =
    Lazy::new(testcontainers::clients::Cli::default);
