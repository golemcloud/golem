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

use crate::components::SupportedServiceLaunch;
use crate::components::rdb::Rdb;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor::spawned::SpawnedWorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use futures::StreamExt;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{Instrument, Level, info};

#[cfg(test)]
use test_r::test;

#[cfg(test)]
#[test]
fn supported_cluster_preflight() {
    use std::collections::HashMap;
    let executable = std::env::current_exe().unwrap();
    let env = |http: &str, grpc: &str| {
        HashMap::from([
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT".into(),
                http.into(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT".into(),
                grpc.into(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__HOST".into(),
                "localhost".into(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT".into(),
                "19020".into(),
            ),
        ])
    };
    let launch = |env| {
        SupportedServiceLaunch::new(
            "worker_executor",
            &executable,
            executable.parent().unwrap().into(),
            env,
        )
    };
    preflight_supported_launches(
        &[launch(env("18301", "19301")), launch(env("18413", "19417"))],
        "localhost",
        19020,
    );
    [
        ("18302", "19301"),
        ("18301", "19302"),
        ("19301", "19302"),
        ("18302", "18301"),
        ("18302", "18302"),
    ]
    .into_iter()
    .for_each(|(http, grpc)| {
        assert!(
            std::panic::catch_unwind(|| preflight_supported_launches(
                &[launch(env("18301", "19301")), launch(env(http, grpc))],
                "localhost",
                19020
            ))
            .is_err()
        );
    });
    [
        ("HTTP_PORT", None),
        ("GRPC__PORT", None),
        ("SHARD_MANAGER__HOST", None),
        ("SHARD_MANAGER__PORT", None),
        ("SHARD_MANAGER__HOST", Some("127.0.0.2")),
        ("SHARD_MANAGER__PORT", Some("19021")),
    ]
    .into_iter()
    .for_each(|(leaf, value)| {
        let mut second = env("18302", "19302");
        let key = format!("GOLEM_ADVANCED__WORKER_EXECUTOR__{leaf}");
        match value {
            Some(value) => {
                second.insert(key, value.into());
            }
            None => {
                second.remove(&key);
            }
        }
        assert!(
            std::panic::catch_unwind(|| preflight_supported_launches(
                &[launch(env("18301", "19301")), launch(second)],
                "localhost",
                19020
            ))
            .is_err()
        );
    });
    let wrong = SupportedServiceLaunch::new(
        "registry_service",
        &executable,
        executable.parent().unwrap().into(),
        HashMap::from([
            (
                "GOLEM_ADVANCED__REGISTRY_SERVICE__HTTP_PORT".into(),
                "18302".into(),
            ),
            (
                "GOLEM_ADVANCED__REGISTRY_SERVICE__GRPC__PORT".into(),
                "19302".into(),
            ),
        ]),
    );
    assert!(
        std::panic::catch_unwind(|| preflight_supported_launches(
            &[launch(env("18301", "19301")), wrong],
            "localhost",
            19020
        ))
        .is_err()
    );
}

fn preflight_supported_launches(
    launches: &[SupportedServiceLaunch],
    shard_host: &str,
    shard_port: u16,
) {
    let mut ports = HashSet::new();
    launches.iter().for_each(|launch| {
        let listeners: [u16; 2] = launch.listeners("worker_executor");
        listeners.into_iter().for_each(|port| {
            assert!(
                ports.insert(port),
                "duplicate supported executor listener {port}"
            )
        });
        [
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__HOST",
                shard_host.to_string(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT",
                shard_port.to_string(),
            ),
        ]
        .into_iter()
        .for_each(|(key, expected)| {
            let actual = launch
                .env
                .iter()
                .find(|(name, _)| name.as_ref() == key)
                .map(|(_, value)| value.as_ref());
            assert_eq!(
                actual,
                Some(expected.as_str()),
                "supported executor shard peer disagrees with registration handle: {key}"
            );
        });
    });
}

pub struct SpawnedWorkerExecutorCluster {
    worker_executors: Vec<Arc<dyn WorkerExecutor>>,
    stopped_indices: Arc<Mutex<HashSet<usize>>>,
}

impl SpawnedWorkerExecutorCluster {
    async fn make_worker_executor(
        executable: PathBuf,
        working_directory: PathBuf,
        http_port: u16,
        grpc_port: u16,
        rdb: Arc<dyn Rdb>,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        registry_service: Arc<dyn RegistryService>,
        environment_state_cache_capacity: Option<usize>,
        oplog_archive_interval: Option<Duration>,
        enable_fs_cache: bool,
        otlp: bool,
    ) -> Arc<dyn WorkerExecutor> {
        Arc::new(
            SpawnedWorkerExecutor::new(
                &executable,
                &working_directory,
                http_port,
                grpc_port,
                rdb,
                shard_manager,
                worker_service,
                verbosity,
                out_level,
                err_level,
                registry_service,
                environment_state_cache_capacity,
                oplog_archive_interval,
                enable_fs_cache,
                otlp,
            )
            .await,
        )
    }

    pub async fn new(
        size: usize,
        base_http_port: u16,
        base_grpc_port: u16,
        executable: &Path,
        working_directory: &Path,
        rdb: Arc<dyn Rdb>,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        registry_service: Arc<dyn RegistryService>,
        environment_state_cache_capacity: Option<usize>,
        oplog_archive_interval: Option<Duration>,
        enable_fs_cache: bool,
        otlp: bool,
    ) -> Self {
        info!("Starting a cluster of golem-worker-executors of size {size}");
        let mut worker_executors = Vec::new();

        for i in 0..size {
            let http_port = base_http_port + i as u16;
            let grpc_port = base_grpc_port + i as u16;

            worker_executors.push(
                Self::make_worker_executor(
                    executable.to_path_buf(),
                    working_directory.to_path_buf(),
                    http_port,
                    grpc_port,
                    rdb.clone(),
                    shard_manager.clone(),
                    worker_service.clone(),
                    verbosity,
                    out_level,
                    err_level,
                    registry_service.clone(),
                    environment_state_cache_capacity,
                    oplog_archive_interval,
                    enable_fs_cache,
                    otlp,
                )
                .in_current_span()
                .await,
            );
        }

        Self::finish(worker_executors, shard_manager).await
    }

    pub(crate) async fn new_supported(
        launches: Vec<SupportedServiceLaunch>,
        shard_manager: Arc<dyn ShardManager>,
        out_level: Level,
        err_level: Level,
    ) -> Self {
        preflight_supported_launches(
            &launches,
            &shard_manager.grpc_host(),
            shard_manager.grpc_port(),
        );
        let worker_executors = futures::stream::iter(launches)
            .then(|launch| async move {
                Arc::new(SpawnedWorkerExecutor::new_supported(launch, out_level, err_level).await)
                    as Arc<dyn WorkerExecutor>
            })
            .collect()
            .await;
        Self::finish(worker_executors, shard_manager).await
    }

    async fn finish(
        worker_executors: Vec<Arc<dyn WorkerExecutor>>,
        shard_manager: Arc<dyn ShardManager>,
    ) -> Self {
        let size = worker_executors.len();
        info!("Waiting for shard manager to see all executors");

        let start = Instant::now();
        let timeout = Duration::from_secs(60);
        loop {
            let routing_table = shard_manager
                .get_routing_table()
                .await
                .expect("Failed to get routing table while waiting for registration");
            if routing_table.all().len() == size {
                break;
            } else {
                if start.elapsed() > timeout {
                    panic!("Failed to wait for all executors to be registered in shard manager");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        Self {
            worker_executors,
            stopped_indices: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

#[async_trait]
impl WorkerExecutorCluster for SpawnedWorkerExecutorCluster {
    fn size(&self) -> usize {
        self.worker_executors.len()
    }

    async fn restart_all(&self) {
        info!("Restarting all worker executors");
        for worker_executor in &self.worker_executors {
            worker_executor.restart().await;
        }
    }

    async fn restart_all_with_extra_env_vars(&self, extra_env_vars: Vec<(String, String)>) {
        info!(
            "Restarting all worker executors with {} extra env var(s)",
            extra_env_vars.len()
        );
        for worker_executor in &self.worker_executors {
            worker_executor
                .restart_with_extra_env_vars(extra_env_vars.clone())
                .await;
        }
    }

    async fn stop(&self, index: usize) {
        let mut stopped = self.stopped_indices.lock().await;
        if !stopped.contains(&index) {
            self.worker_executors[index]
                .kill_and_wait(tokio::time::Instant::now() + Duration::from_secs(60))
                .await
                .expect("failed to stop worker executor");
            stopped.insert(index);
        }
    }

    async fn start(&self, index: usize) {
        if self.stopped_indices().await.contains(&index) {
            self.worker_executors[index].restart().await;
            self.stopped_indices.lock().await.remove(&index);
        }
    }

    async fn pause(&self, index: usize) {
        self.worker_executors[index].pause().await;
    }

    async fn resume(&self, index: usize) {
        self.worker_executors[index].resume().await;
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor>> {
        self.worker_executors.to_vec()
    }

    async fn stopped_indices(&self) -> Vec<usize> {
        self.stopped_indices.lock().await.iter().copied().collect()
    }

    async fn started_indices(&self) -> Vec<usize> {
        let all_indices = HashSet::from_iter(0..self.worker_executors.len());
        let stopped_indices = self.stopped_indices.lock().await;
        all_indices.difference(&stopped_indices).copied().collect()
    }

    async fn is_running(&self) -> bool {
        for executor in &self.worker_executors {
            if !executor.is_running().await {
                return false;
            }
        }
        true
    }
}
