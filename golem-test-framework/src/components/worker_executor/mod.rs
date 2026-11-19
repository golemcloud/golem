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

use super::rdb::Rdb;
use super::registry_service::RegistryService;
use super::shard_manager::ShardManager;
use super::worker_service::WorkerService;
use super::{EnvVarBuilder, wait_for_startup_grpc};
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;

pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerExecutor: Send + Sync {
    fn grpc_host(&self) -> String;

    fn grpc_port(&self) -> u16;

    /// Hard kill and reap the owned child before the deadline. Unsupported
    /// executors fail closed; absence of a child handle is not proof of exit.
    async fn kill_and_wait(&self, _deadline: tokio::time::Instant) -> anyhow::Result<()> {
        anyhow::bail!("kill-and-wait requires a spawned worker executor")
    }

    fn is_reaped(&self) -> bool {
        false
    }

    /// Holds the lifecycle lock only if this process has been reaped, preventing
    /// concurrent restart while test code alters derived storage.
    async fn lock_reaped(&self) -> anyhow::Result<Box<dyn Send + Sync>> {
        anyhow::bail!("storage mutation requires a reaped spawned executor")
    }

    /// The process ID of the running executor process, or `None` when it is not running or the
    /// executor is not a spawned process.
    fn pid(&self) -> Option<u32> {
        None
    }

    /// URL and generation of the spawned process, for reset-aware metrics.
    fn metrics_endpoint(&self) -> Option<(String, u64)> {
        None
    }

    async fn restart(&self);

    /// Restarts this worker executor with `extra_env_vars` merged into
    /// the spawned child process's environment **for this restart only**.
    ///
    /// Default implementation panics: only `SpawnedWorkerExecutor` supports
    /// this. The parent-side `WorkerExecutorClusterControl::restart_all_with_env_vars`
    /// HostedRpc handler uses this to apply per-test config overrides without
    /// mutating the parent test runner's process-wide environment (which would
    /// be unsound in a multi-threaded process).
    async fn restart_with_extra_env_vars(&self, _extra_env_vars: Vec<(String, String)>) {
        panic!(
            "WorkerExecutor::restart_with_extra_env_vars is only supported by \
             SpawnedWorkerExecutor; the default implementation refuses to silently \
             discard the requested env overrides."
        );
    }

    /// Freezes this worker executor's process in place (SIGSTOP) without killing it: it keeps its
    /// sockets, its memory and every lease it believes it holds, but answers nothing until
    /// [`WorkerExecutor::resume`]. This is how an executor is made to look dead to the rest of
    /// the cluster while it still thinks it owns its shards, which a kill cannot do.
    ///
    /// Default implementation panics: only `SpawnedWorkerExecutor` owns a process to freeze.
    async fn pause(&self) {
        panic!("WorkerExecutor::pause is only supported by SpawnedWorkerExecutor");
    }

    /// Thaws a process frozen by [`WorkerExecutor::pause`] (SIGCONT). It carries on from exactly
    /// where it stopped.
    ///
    /// Default implementation panics: only `SpawnedWorkerExecutor` owns a process to thaw.
    async fn resume(&self) {
        panic!("WorkerExecutor::resume is only supported by SpawnedWorkerExecutor");
    }

    async fn is_running(&self) -> bool;
}

async fn wait_for_startup(
    host: &str,
    grpc_port: u16,
    timeout: Duration,
    child: Option<&mut Child>,
) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-executor", timeout, child).await
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    shard_manager: &Arc<dyn ShardManager>,
    worker_service: &Arc<dyn WorkerService>,
    rdb: &Arc<dyn Rdb>,
    registry_service: &Arc<dyn RegistryService>,
    environment_state_cache_capacity: Option<usize>,
    oplog_archive_interval: Option<Duration>,
    verbosity: Level,
    enable_fs_cache: bool,
    otlp: bool,
) -> HashMap<String, String> {
    let mut env = EnvVarBuilder::golem_service(verbosity)
        .with_str("ENVIRONMENT", "local")
        .with_str("WASMTIME_BACKTRACE_DETAILS", "1")
        .with(
            "GOLEM__DATA_DISK__BLOB_ROOT",
            std::env::temp_dir()
                .join("ittest-local-object-store/golem")
                .to_string_lossy()
                .to_string(),
        )
        .with_str("GOLEM__BLOB_STORE", "data_disk")
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__PUBLIC_WORKER_API__HOST",
            &worker_service.grpc_host(),
        )
        .with(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__PUBLIC_WORKER_API__PORT",
            worker_service.gprc_port().to_string(),
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__REGISTRY_SERVICE__HOST",
            &registry_service.grpc_host(),
        )
        .with(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__REGISTRY_SERVICE__PORT",
            registry_service.grpc_port().to_string(),
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__COMPILED_COMPONENT_SERVICE__TYPE",
            "Enabled",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__HOST",
            &shard_manager.grpc_host(),
        )
        .with(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT",
            shard_manager.grpc_port().to_string(),
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__RETRIES__MAX_ATTEMPTS",
            "5",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__RETRIES__MIN_DELAY",
            "100ms",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__RETRIES__MAX_DELAY",
            "2s",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__RETRIES__MULTIPLIER",
            "2",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__LIMITS__FUEL_TO_BORROW",
            "100000",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__RESOURCE_USAGE_METERING__COMPUTE",
            "true",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__AGENT_WEBHOOKS_SERVICE__USE_HTTPS_FOR_WEBHOOK_URL",
            "false",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__QUOTA_SERVICE__INLINE_WAIT_THRESHOLD",
            "30s",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__QUOTA_SERVICE__RENEWAL_INTERVAL",
            "3s",
        )
        .with_str(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__QUOTA_SERVICE__RENEWAL_THRESHOLD",
            "55s",
        ) // shard manager sets 60s lease duration -> renew every 5s
        .with(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT",
            grpc_port.to_string(),
        )
        .with(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT",
            http_port.to_string(),
        )
        .with(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__ENGINE__ENABLE_FS_CACHE",
            enable_fs_cache.to_string(),
        )
        .with_optional_otlp("worker_executor", otlp)
        .build();

    if let Some(environment_state_cache_capacity) = environment_state_cache_capacity {
        env.insert(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__ENVIRONMENT_STATE_SERVICE__CACHE_CAPACITY"
                .to_string(),
            environment_state_cache_capacity.to_string(),
        );
    }

    if let Some(oplog_archive_interval) = oplog_archive_interval {
        env.insert(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__OPLOG__ARCHIVE_INTERVAL".to_string(),
            serde_json::to_string(&humantime_serde::Serde::from(oplog_archive_interval)).unwrap(),
        );
    }

    env.extend(rdb.info().env("golem_worker_executor", false));

    env
}
