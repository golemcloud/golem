pub mod cloud_service;
pub mod component_compilation_service;
pub mod component_service;
pub mod rdb;
pub mod redis;
pub mod redis_monitor;
pub mod shard_manager;
pub mod worker_executor;
pub mod worker_executor_cluster;
pub mod worker_service;

use crate::components::cloud_service::CloudService;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use golem_api_grpc::proto::grpc::health::v1::health_check_response::ServingStatus;
use golem_api_grpc::proto::grpc::health::v1::HealthCheckRequest;
use once_cell::sync::Lazy;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, error, info, trace, warn, Level};

const NETWORK: &str = "golem_test_network";
pub const ROOT_TOKEN: &str = "2A354594-7A63-4091-A46B-CC58D379F677";

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

async fn wait_for_startup_grpc(host: &str, grpc_port: u16, name: &str, timeout: Duration) {
    info!(
        "Waiting for {name} start on host {host}:{grpc_port}, timeout: {}s",
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

pub trait TestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static>;
    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static>;
    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static>;
    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static>;
    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static>;
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>;
    fn cloud_service(&self) -> Arc<dyn CloudService + Send + Sync + 'static>;

    fn kill_all(&self) {
        self.worker_executor_cluster().kill_all();
        self.worker_service().kill();
        self.component_compilation_service().kill();
        self.component_service().kill();
        self.shard_manager().kill();
        self.cloud_service().kill();
        self.rdb().kill();
        self.redis_monitor().kill();
        self.redis().kill();
    }
}

static DOCKER: Lazy<testcontainers::clients::Cli> =
    Lazy::new(testcontainers::clients::Cli::default);
