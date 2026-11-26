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

use super::*;
use crate::services::golem_config::{
    FilesystemStorageMode, IndexedStorageInMemoryConfig, KeyValueStorageInMemoryConfig,
};
use crate::workerctx::default::Context;
use golem_service_base::storage::blob::memory::InMemoryS3BlobStorageFactory;
use std::time::Duration;
use test_r::test;

#[test]
#[test_r::timeout("5s")]
async fn s3_startup_failure_stops_executor_before_engine_and_services() {
    let config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::InMemory(KeyValueStorageInMemoryConfig {}),
        indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
        scheduler_storage: SchedulerStorageConfig::InMemory(Default::default()),
        blob_storage: BlobStorageConfig::S3(golem_service_base::config::S3BlobStorageConfig {
            region: "us-east-1".to_string(),
            aws_endpoint_url: Some("http://s3.test".to_string()),
            oplog_payload_bucket: "executor-startup-only".to_string(),
            object_prefix: "objects-not-the-bucket".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let failure = std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "selected bucket transport refusal",
    );
    let factory = InMemoryS3BlobStorageFactory::failed(failure);
    let mut tasks = JoinSet::new();
    let error = create_worker_executor_impl::<Context, _>(
        config,
        &crate::bootstrap::ServerBootstrap,
        Handle::current(),
        &Arc::new(LazyWorkerActivator::new()),
        services::shutdown::Shutdown::new(),
        &mut tasks,
        factory,
    )
    .await
    .err()
    .expect("S3 failure must stop executor construction");
    assert_eq!(
        error.to_string(),
        "S3 startup check failed for bucket executor-startup-only"
    );
    assert_eq!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .unwrap()
            .kind(),
        std::io::ErrorKind::ConnectionRefused
    );
    assert_eq!(
        error.root_cause().to_string(),
        "selected bucket transport refusal"
    );
    assert!(tasks.is_empty(), "services must not start after S3 fails");
}

#[test]
#[test_r::timeout("10s")]
async fn failed_bootstrap_cleanup_is_bounded_and_leaves_caller_tasks_running() {
    let mut caller_tasks = JoinSet::<Result<(), anyhow::Error>>::new();
    let (started, running) = tokio::sync::oneshot::channel();
    let (release_caller, released_caller) = tokio::sync::oneshot::channel();
    let caller_task = caller_tasks.spawn(async move {
        started.send(()).unwrap();
        released_caller.await.unwrap();
        Ok(())
    });
    running.await.unwrap();

    let shutdown = services::shutdown::Shutdown::new();
    let (release_cleanup, released_cleanup) = tokio::sync::oneshot::channel();
    let token = shutdown.token();
    let cleanup = shutdown.spawn(async move {
        token.cancelled().await;
        released_cleanup.await.unwrap();
    });
    let completed = tokio::time::timeout(
        Duration::from_secs(1),
        shutdown.cancel_and_wait_for_tracked(Duration::from_millis(10)),
    )
    .await
    .expect("failed-bootstrap cleanup must return after its grace");
    assert!(!completed);
    assert!(shutdown.token().is_cancelled());
    assert!(
        !cleanup.is_finished(),
        "a timeout must not abandon owned cleanup"
    );
    assert!(
        !caller_task.is_finished(),
        "executor failure must not abort caller tasks"
    );

    release_cleanup.send(()).unwrap();
    shutdown.join_tracked().await;
    cleanup.await.unwrap();
    release_caller.send(()).unwrap();
    caller_tasks.join_next().await.unwrap().unwrap().unwrap();
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::services::NoAdditionalDeps;
    use crate::services::agent_filesystem::file_creation_mask_for_test::{
        thread_file_creation_mask, with_private_file_creation_mask,
    };
    use test_r::test;

    /// Records the file mode creation mask of the thread when bootstrap starts the active agents,
    /// and starts none.
    #[derive(Default)]
    struct MaskRecordingBootstrap {
        mask: std::sync::Mutex<Option<libc::mode_t>>,
    }

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl Bootstrap<Context> for MaskRecordingBootstrap {
        fn create_additional_deps(
            &self,
            _registry_service: Arc<dyn RegistryService>,
        ) -> NoAdditionalDeps {
            NoAdditionalDeps {}
        }

        async fn create_active_agents(
            &self,
            _golem_config: &GolemConfig,
            _initial_files_service: Arc<InitialAgentFilesService>,
            _shutdown_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<Arc<ActiveAgents<Context>>> {
            *self.mask.lock().unwrap() = Some(thread_file_creation_mask());
            Err(anyhow!("the test starts no active agents"))
        }
    }

    /// Bootstrap runs on a thread with its own filesystem attributes and the mask 0o227. The mask
    /// must be 0o027 when the active agents start, and stay 0o027 after.
    #[test]
    fn executor_bootstrap_clears_only_bit_0o200_before_the_agent_filesystems_start() {
        let (started, recorded, after) = with_private_file_creation_mask(0o227, || {
            let bootstrap = MaskRecordingBootstrap::default();
            let started = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(start_active_agents(
                    &bootstrap,
                    &GolemConfig::default(),
                    in_memory_initial_files_service(),
                    tokio_util::sync::CancellationToken::new(),
                ))
                .is_ok();
            let recorded = *bootstrap.mask.lock().unwrap();
            (started, recorded, thread_file_creation_mask())
        });

        assert!(!started, "the recording bootstrap starts no active agents");
        assert_eq!(
            recorded,
            Some(0o027),
            "the mask 227 must be 27 when the agent filesystems start"
        );
        assert_eq!(after, 0o027, "the mask must stay 27, and it is {after:o}");
    }
}

fn in_memory_initial_files_service() -> Arc<InitialAgentFilesService> {
    Arc::new(InitialAgentFilesService::new(Arc::new(
        golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
    )))
}

async fn directory_cleanup_services(
    root: &std::path::Path,
    shutdown: &services::shutdown::Shutdown,
) -> (
    Arc<services::agent_filesystem::AgentFilesystems>,
    Arc<AgentFilesystemSnapshots>,
) {
    let settings = services::golem_config::FilesystemStorageConfig {
        mode: FilesystemStorageMode::Directory { root: root.into() },
        ..Default::default()
    };
    let filesystems = Arc::new(
        services::agent_filesystem::AgentFilesystems::new(
            &settings,
            in_memory_initial_files_service(),
        )
        .await
        .unwrap(),
    );
    let snapshots = AgentFilesystemSnapshots::bind(
        &services::golem_config::FilesystemSnapshotsConfig::default(),
        StoreSource::configured(
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new()),
            VolumeRoom::Pressure {
                volume: filesystems.volume().clone(),
                pressure: filesystems.pressure_policy().clone(),
            },
        ),
        filesystems.volume().copies_on_write(),
        shutdown,
    )
    .unwrap();
    (filesystems, snapshots)
}

#[test]
#[test_r::timeout("30s")]
async fn executor_directory_cleanup_waits_for_invocation_drain_and_is_joined_by_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let shutdown = services::shutdown::Shutdown::new();
    let (filesystems, snapshots) = directory_cleanup_services(root.path(), &shutdown).await;
    let loops = InvocationLoops::new(shutdown.token());
    let (entered, started) = tokio::sync::oneshot::channel();
    let (draining, drain_started) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    loops.spawn(
        async move {
            let _ = entered.send(());
            std::future::pending::<()>().await;
        },
        move || {
            Box::pin(async move {
                let _ = draining.send(());
                let _ = released.await;
            })
        },
    );
    started.await.unwrap();
    filesystems.track_shutdown(&shutdown, loops, snapshots);
    shutdown.cancel();
    drain_started.await.unwrap();
    let waiting = shutdown.wait_for_tracked(std::time::Duration::from_secs(10));
    tokio::pin!(waiting);
    assert!(futures::poll!(&mut waiting).is_pending());
    assert!(root.path().join(".scratch").is_dir());
    assert!(root.path().join(".initial-files").is_dir());
    release.send(()).unwrap();
    assert!(waiting.await);
    assert!(!root.path().join(".scratch").exists());
    assert!(!root.path().join(".initial-files").exists());
}

#[test]
#[test_r::timeout("10s")]
async fn failed_bootstrap_drops_run_details_and_releases_cleanup_owners() {
    let root = tempfile::tempdir().unwrap();
    let shutdown = services::shutdown::Shutdown::new();
    let (filesystems, snapshots) = directory_cleanup_services(root.path(), &shutdown).await;
    let released = Arc::downgrade(&filesystems);
    let loops = InvocationLoops::new(shutdown.token());
    let failed: anyhow::Result<RunDetails> = {
        let _run_details = RunDetails {
            http_port: 0,
            grpc_port: 0,
            _epoch: EpochTicker::start(std::time::Duration::from_secs(60), || {}),
            shutdown: shutdown.clone(),
            leak_detector: std::sync::Weak::new(),
            invocation_loops: loops.clone(),
        };
        filesystems.track_shutdown(&shutdown, loops, snapshots);
        drop(filesystems);
        Err(anyhow!("server bind failed"))
    };
    assert!(failed.is_err());
    assert!(shutdown.token().is_cancelled());
    shutdown.join_tracked().await;
    assert!(released.upgrade().is_none());
    assert!(!root.path().join(".scratch").exists());
    assert!(!root.path().join(".initial-files").exists());
}

/// The snapshot service gets the storage mode of the agent filesystems: on storage without
/// copy-on-write copies it refuses enabled snapshots.
#[test]
async fn the_snapshot_service_refuses_enabled_snapshots_on_storage_without_copy_on_write() {
    let root = tempfile::tempdir().unwrap();
    let key: Box<str> = "00".repeat(64).into_boxed_str();
    let mut golem_config = GolemConfig::default();
    golem_config.filesystem_storage.mode = FilesystemStorageMode::Directory {
        root: root.path().into(),
    };
    golem_config.filesystem_snapshots =
        services::golem_config::FilesystemSnapshotsConfig::Enabled(Box::new(
            services::golem_config::FilesystemSnapshotStoreConfig::new(
                &key,
                std::time::Duration::from_secs(60),
                1,
                1,
            )
            .unwrap(),
        ));
    let shutdown = services::shutdown::Shutdown::new();
    let active_agents = Arc::new(
        ActiveAgents::<Context>::new(
            &golem_config.active_agents,
            &golem_config.memory,
            &golem_config.filesystem_storage,
            in_memory_initial_files_service(),
            &golem_config.agent_status_flush,
            shutdown.token(),
        )
        .await
        .unwrap(),
    );

    let bound = bind_agent_filesystem_snapshots(
        &golem_config,
        None,
        Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new()),
        &active_agents,
        &shutdown,
    );

    let error = bound
        .err()
        .expect("enabled snapshots on storage without copy-on-write must be refused");
    assert!(
            format!("{error:#}").contains(
                "filesystem snapshots require storage with copy-on-write copies (XFS with reflink, or Apfs for local development)"
            ),
            "{error:#}"
        );
    shutdown.token().cancel();
}
