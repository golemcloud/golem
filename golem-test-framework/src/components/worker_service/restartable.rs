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

use crate::components::new_reqwest_client_with_tracing;
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::config::{EnvBasedTestDependenciesConfig, TestDependencies, service_executable};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;
use tracing::Level;

/// A worker service owned by one test, which the test can kill and start again on the same ports.
///
/// It uses the shared shard manager, database, Redis and registry service of the test
/// dependencies, but it is a separate process on its own free ports. Killing or restarting it
/// never affects the worker service that other tests share. Clients that keep its address
/// reconnect to the new process after [`Self::restart`].
pub struct RestartableWorkerService {
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    mcp_port: u16,
    executable: PathBuf,
    working_directory: PathBuf,
    shard_manager: Arc<dyn ShardManager>,
    rdb: Arc<dyn Rdb>,
    redis: Arc<dyn Redis>,
    registry_service: Arc<dyn RegistryService>,
    verbosity: Level,
    out_level: Level,
    err_level: Level,
    process: Mutex<Option<Arc<SpawnedWorkerService>>>,
    base_http_client: OnceCell<reqwest_middleware::ClientWithMiddleware>,
}

impl RestartableWorkerService {
    pub async fn start(deps: &impl TestDependencies) -> anyhow::Result<Self> {
        let config = EnvBasedTestDependenciesConfig::new();
        let [http_port, grpc_port, custom_request_port, mcp_port] = free_ports().await?;
        let service = Self {
            http_port,
            grpc_port,
            custom_request_port,
            mcp_port,
            executable: service_executable(&config.golem_repo_root, "golem-worker-service"),
            working_directory: config.golem_repo_root.join("golem-worker-service"),
            shard_manager: deps.shard_manager(),
            rdb: deps.rdb(),
            redis: deps.redis(),
            registry_service: deps.registry_service(),
            verbosity: config.default_verbosity(),
            out_level: config.default_stdout_level(),
            err_level: config.default_stderr_level(),
            process: Mutex::new(None),
            base_http_client: OnceCell::new(),
        };
        service.spawn().await;
        Ok(service)
    }

    /// The process ID of the running worker service, or `None` while it is killed.
    pub fn pid(&self) -> Option<u32> {
        self.process
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|process| process.pid())
    }

    /// Kills the running process, if any, and starts a new one on the same ports. Returns when
    /// the new process accepts requests on all of its ports.
    pub async fn restart(&self) {
        self.kill().await;
        self.spawn().await;
    }

    async fn spawn(&self) {
        let process = SpawnedWorkerService::new(
            &self.executable,
            &self.working_directory,
            self.http_port,
            self.grpc_port,
            self.custom_request_port,
            self.mcp_port,
            &self.shard_manager,
            &self.rdb,
            &self.redis,
            self.verbosity,
            self.out_level,
            self.err_level,
            &self.registry_service,
            true,
            false,
        )
        .await;
        *self.process.lock().unwrap() = Some(Arc::new(process));
    }
}

/// Four free loopback ports for the HTTP, gRPC, custom request and MCP listeners. The worker
/// service always binds MCP two ports above the custom request port, so that pair is reserved
/// together.
async fn free_ports() -> anyhow::Result<[u16; 4]> {
    loop {
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let grpc = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let custom = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let custom_port = custom.local_addr()?.port();
        let Some(mcp_port) = custom_port.checked_add(2) else {
            continue;
        };
        let Ok(mcp) = tokio::net::TcpListener::bind(("127.0.0.1", mcp_port)).await else {
            continue;
        };
        let ports = [
            http.local_addr()?.port(),
            grpc.local_addr()?.port(),
            custom_port,
            mcp.local_addr()?.port(),
        ];
        drop((http, grpc, custom, mcp));
        return Ok(ports);
    }
}

#[async_trait]
impl WorkerService for RestartableWorkerService {
    fn http_host(&self) -> String {
        "localhost".to_string()
    }
    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }
    fn gprc_port(&self) -> u16 {
        self.grpc_port
    }

    fn custom_request_host(&self) -> String {
        "localhost".to_string()
    }
    fn custom_request_port(&self) -> u16 {
        self.custom_request_port
    }

    fn mcp_port(&self) -> u16 {
        self.mcp_port
    }

    async fn base_http_client(&self) -> reqwest_middleware::ClientWithMiddleware {
        self.base_http_client
            .get_or_init(|| async { new_reqwest_client_with_tracing() })
            .await
            .clone()
    }

    /// Kills the process and waits for its exit. [`Self::restart`] starts the next process on the
    /// same ports.
    async fn kill(&self) {
        let process = self.process.lock().unwrap().take();
        if let Some(process) = process {
            process.kill().await;
        }
    }
}
