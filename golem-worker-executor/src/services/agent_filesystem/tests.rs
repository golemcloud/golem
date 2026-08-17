use super::*;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::component::{AgentFilePath, AgentFilePermissions, ComponentId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use golem_common::widen_infallible;
use golem_service_base::replayable_stream::ReplayableStream as _;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::test;
use wasmtime_wasi::StreamResult;
use wasmtime_wasi::p2::{OutputStream, Pollable};

struct DelayedOutputStream {
    ready: Arc<tokio::sync::Semaphore>,
    cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl OutputStream for DelayedOutputStream {
    fn write(&mut self, _bytes: Bytes) -> StreamResult<()> {
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(1024)
    }

    async fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Pollable for DelayedOutputStream {
    async fn ready(&mut self) {
        self.ready
            .acquire()
            .await
            .expect("test readiness semaphore closed")
            .forget();
    }
}

fn agent_id() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId::from_agent_name_string(ComponentId::new(), "agent").unwrap(),
    )
}

async fn file_loader_with_content(
    environment_id: EnvironmentId,
    cache_parent: Option<&Path>,
    content: &[u8],
) -> (
    Arc<FileLoader>,
    golem_common::model::agent::AgentFileContentHash,
) {
    let service = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let hash = service
        .put_if_not_exists(
            environment_id,
            content
                .to_vec()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await
        .unwrap();
    (
        Arc::new(FileLoader::new(service, cache_parent).unwrap()),
        hash,
    )
}

fn initial_file(
    content_hash: golem_common::model::agent::AgentFileContentHash,
    path: &str,
    permissions: AgentFilePermissions,
    size: u64,
) -> InitialAgentFile {
    InitialAgentFile {
        content_hash,
        path: AgentFilePath::from_abs_str(path).unwrap(),
        permissions,
        size,
    }
}

#[test]
fn default_object_limit_policy_resolves_storage_levels() {
    let policy = FilesystemObjectLimitPolicyConfig::default();

    assert_eq!(
        policy
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: 128 * 1024 * 1024,
            },)
            .unwrap(),
        ResolvedAgentFilesystemLimits {
            allocated_bytes: 128 * 1024 * 1024,
            filesystem_objects: 8_192,
            filesystem_object_limit_policy_version: 2,
        }
    );
    assert_eq!(
        policy
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: 384 * 1024 * 1024,
            },)
            .unwrap()
            .filesystem_objects,
        12_288
    );
    assert_eq!(
        policy
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: 1024 * 1024 * 1024,
            },)
            .unwrap()
            .filesystem_objects,
        32_768
    );
}

#[test]
fn object_limit_policy_rejects_unrepresentable_inputs() {
    let policy = FilesystemObjectLimitPolicyConfig::default();

    assert!(
        policy
            .resolve(AgentFilesystemStorageLimit { allocated_bytes: 0 })
            .is_err()
    );
    let overflowing = FilesystemObjectLimitPolicyConfig {
        objects_per_gib: u64::MAX,
        maximum_objects: u64::MAX,
        ..policy.clone()
    };
    assert!(
        overflowing
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: u64::MAX,
            })
            .is_err()
    );

    let invalid = FilesystemObjectLimitPolicyConfig {
        objects_per_gib: 0,
        ..policy
    };
    assert!(invalid.validate().is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn managed_backend_fails_closed_on_non_xfs() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };

    let error = match AgentFilesystems::new(&settings) {
        Ok(_) => panic!("managed backend unexpectedly accepted a non-XFS root"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("validate managed XFS root"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
async fn managed_xfs_owns_observes_and_cleans_project_filesystem() {
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let settings = FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root.clone()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();

    let second_owner = AgentFilesystems::new(&settings);
    assert!(second_owner.is_err());

    let escaped_id = agent_id();
    let outside = tempfile::tempdir().unwrap();
    let environment_link = root.join(escaped_id.environment_id.to_string());
    std::os::unix::fs::symlink(outside.path(), &environment_link).unwrap();
    assert!(filesystems.create_owned_empty(&escaped_id).await.is_err());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    std::fs::remove_file(environment_link).unwrap();

    let stale_file_id = agent_id();
    let backend = Arc::clone(filesystems.managed_xfs.as_ref().unwrap());
    let environment = stale_file_id.environment_id.to_string();
    let component = stale_file_id.agent_id.component_id.to_string();
    let agent = stale_file_id.agent_id.agent_name_encoded();
    let owner = PathBuf::from(&environment).join(&component).join(&agent);
    let parent = backend.open_agent_parent(&environment, &component).unwrap();
    let parent_path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let stale_file = parent_path.join(&agent);
    let staging = parent_path.join(format!("{agent}.staging"));
    std::fs::create_dir(&staging).unwrap();
    let stale_project = backend.reserve_project(&owner).unwrap();
    let staging_directory = File::open(&staging).unwrap();
    backend
        .assign_project(&staging_directory, stale_project)
        .unwrap();
    std::fs::write(staging.join("file"), b"stale").unwrap();
    std::fs::rename(staging.join("file"), &stale_file).unwrap();
    drop(staging_directory);
    std::fs::remove_dir(staging).unwrap();
    drop(parent);

    let stale_file_replacement = filesystems
        .create_owned_empty(&stale_file_id)
        .await
        .unwrap();
    assert!(stale_file_replacement.path().is_dir());
    stale_file_replacement.close_and_delete().await.unwrap();
    assert_eq!(
        backend.usage(stale_project).unwrap(),
        AgentFilesystemUsage {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );

    let capacity = filesystems.capacity().await.unwrap();
    assert!(capacity.total_bytes > 0);
    assert!(capacity.available_bytes <= capacity.total_bytes);
    assert!(capacity.total_filesystem_objects > 0);
    assert!(capacity.available_filesystem_objects <= capacity.total_filesystem_objects);

    let materialized_id = agent_id();
    let content = vec![0x5a; 8192];
    let (file_loader, content_hash) = file_loader_with_content(
        materialized_id.environment_id,
        filesystems.initial_file_cache_root(),
        &content,
    )
    .await;
    let cached_source = file_loader
        .get_source(
            materialized_id.environment_id,
            content_hash,
            content.len() as u64,
        )
        .await
        .unwrap();
    let managed_backend = Arc::clone(filesystems.managed_xfs.as_ref().unwrap());
    assert_eq!(
        managed_backend
            .project_id(&File::open(cached_source.path()).unwrap())
            .unwrap(),
        None,
        "the shared cache source must not inherit an agent project"
    );
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: materialized_id.clone(),
            initial_files: vec![
                initial_file(
                    content_hash,
                    "/immutable-a",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/immutable-b",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();
    let path = filesystem.path().to_path_buf();
    let (backend, project_id) = match &filesystem.storage {
        AgentFilesystemStorage::Managed {
            backend,
            project_id,
            ..
        } => (Arc::clone(backend), *project_id),
        AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
    };
    let materialized_usage = filesystem.usage().await.unwrap().unwrap();
    assert!(materialized_usage.allocated_bytes >= 3 * 8192);
    assert!(materialized_usage.filesystem_objects >= 4);
    assert_eq!(std::fs::read(path.join("immutable-a")).unwrap(), content);
    assert_eq!(std::fs::read(path.join("immutable-b")).unwrap(), content);
    assert_eq!(std::fs::read(path.join("writable")).unwrap(), content);
    let immutable_a = File::open(path.join("immutable-a")).unwrap();
    let immutable_b = File::open(path.join("immutable-b")).unwrap();
    let writable = File::open(path.join("writable")).unwrap();
    assert_eq!(backend.project_id(&immutable_a).unwrap(), Some(project_id));
    assert_eq!(backend.project_id(&immutable_b).unwrap(), Some(project_id));
    assert_eq!(backend.project_id(&writable).unwrap(), Some(project_id));
    drop((immutable_a, immutable_b, writable));

    filesystem
        .runtime()
        .set_allocated_byte_limit(AgentFilesystemStorageLimit {
            allocated_bytes: 128 * 1024 * 1024,
        })
        .await
        .unwrap();

    filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            materialized_id.environment_id,
            &[
                initial_file(
                    content_hash,
                    "/immutable-a",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/immutable-c",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
        )
        .await
        .unwrap();
    assert!(!path.join("immutable-b").exists());
    assert_eq!(std::fs::read(path.join("immutable-c")).unwrap(), content);
    let immutable_c = File::open(path.join("immutable-c")).unwrap();
    assert_eq!(backend.project_id(&immutable_c).unwrap(), Some(project_id));
    drop(immutable_c);

    let usage_before_cow = filesystem.usage().await.unwrap().unwrap();
    std::fs::write(path.join("writable"), vec![0x6c; content.len()]).unwrap();
    assert_eq!(std::fs::read(path.join("immutable-a")).unwrap(), content);
    assert_eq!(std::fs::read(path.join("immutable-c")).unwrap(), content);
    let usage_after_cow = filesystem.usage().await.unwrap().unwrap();
    assert!(usage_after_cow.allocated_bytes >= usage_before_cow.allocated_bytes);

    use std::io::{Seek, SeekFrom, Write};
    let usage_before_sparse = filesystem.usage().await.unwrap().unwrap();
    let sparse_path = path.join("sparse");
    let mut sparse = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&sparse_path)
        .unwrap();
    sparse.seek(SeekFrom::Start(4 * 1024 * 1024)).unwrap();
    sparse.write_all(&[0x7d]).unwrap();
    sparse.sync_all().unwrap();
    let usage_after_sparse = filesystem.usage().await.unwrap().unwrap();
    assert_eq!(
        std::fs::metadata(&sparse_path).unwrap().len(),
        4 * 1024 * 1024 + 1
    );
    assert!(usage_after_sparse.allocated_bytes > usage_before_sparse.allocated_bytes);
    assert!(
        usage_after_sparse.allocated_bytes - usage_before_sparse.allocated_bytes < 1024 * 1024,
        "sparse logical extension must be charged by physical allocation"
    );

    let dense_path = path.join("dense");
    let mut dense = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&dense_path)
        .unwrap();
    dense.write_all(&vec![0x4e; 4096]).unwrap();
    dense.sync_all().unwrap();
    let usage = filesystem.usage().await.unwrap().unwrap();
    assert!(usage.allocated_bytes > 4096);
    let limit_exceeded = Arc::new(AtomicBool::new(false));
    filesystem.runtime().set_limit_exceeded_callback(Some({
        let limit_exceeded = Arc::clone(&limit_exceeded);
        Arc::new(move |exceeded| {
            let limit_exceeded = Arc::clone(&limit_exceeded);
            Box::pin(async move {
                if exceeded {
                    limit_exceeded.store(true, Ordering::Release);
                }
            })
        })
    }));
    filesystem
        .runtime()
        .set_allocated_byte_limit(AgentFilesystemStorageLimit {
            allocated_bytes: usage.allocated_bytes,
        })
        .await
        .unwrap();
    assert!(!limit_exceeded.load(Ordering::Acquire));
    dense.seek(SeekFrom::Start(0)).unwrap();
    dense.write_all(&vec![0x5f; 4096]).unwrap();
    dense.sync_all().unwrap();
    assert_eq!(
        filesystem.usage().await.unwrap().unwrap().allocated_bytes,
        usage.allocated_bytes,
        "overwriting allocated blocks at quota equality must not consume capacity"
    );
    let allocation_error = backend
        .materialize_initial_file(
            &path,
            project_id,
            cached_source.path(),
            &path.join("exact-limit-allocation"),
            false,
        )
        .expect_err("allocating at the exact byte limit must be denied");
    assert!(
        matches!(
            allocation_error.kind(),
            std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
        ),
        "unexpected exact-limit allocation error: {allocation_error:?}"
    );
    drop((sparse, dense));
    filesystem
        .runtime()
        .set_allocated_byte_limit(AgentFilesystemStorageLimit {
            allocated_bytes: usage.allocated_bytes - 4096,
        })
        .await
        .unwrap();
    assert!(limit_exceeded.load(Ordering::Acquire));

    filesystem.close_and_delete().await.unwrap();
    assert!(!path.exists());
    assert_eq!(
        backend.usage(project_id).unwrap(),
        AgentFilesystemUsage {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );

    let over_limit_id = agent_id();
    let over_limit_content = vec![0x7b; 8192];
    let (over_limit_loader, over_limit_hash) = file_loader_with_content(
        over_limit_id.environment_id,
        filesystems.initial_file_cache_root(),
        &over_limit_content,
    )
    .await;
    let error = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: over_limit_id,
            initial_files: vec![initial_file(
                over_limit_hash,
                "/over-limit-initial-file",
                AgentFilePermissions::ReadOnly,
                over_limit_content.len() as u64,
            )],
            file_loader: over_limit_loader,
            resource_limits: Some(Arc::new(AtomicResourceEntry::new(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                4096,
                u64::MAX,
            ))),
            limit_exceeded: None,
        })
        .await;
    assert!(
        error.is_err(),
        "initial files above the installed byte limit must prevent startup"
    );

    let object_limited = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let (object_backend, object_project) = match &object_limited.storage {
        AgentFilesystemStorage::Managed {
            backend,
            project_id,
            ..
        } => (Arc::clone(backend), *project_id),
        AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
    };
    object_backend
        .install_project_limits(
            object_project,
            ResolvedAgentFilesystemLimits {
                allocated_bytes: 128 * 1024 * 1024,
                filesystem_objects: 2,
                filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
            },
        )
        .unwrap();
    let object_path = object_limited.path().join("object");
    std::fs::write(&object_path, []).unwrap();
    std::fs::hard_link(&object_path, object_limited.path().join("alias")).unwrap();
    assert_eq!(
        object_limited
            .usage()
            .await
            .unwrap()
            .unwrap()
            .filesystem_objects,
        2
    );
    let object_error = std::fs::write(object_limited.path().join("exhausted"), [])
        .expect_err("a new inode must exceed the project object limit");
    assert_eq!(
        object_error.raw_os_error(),
        Some(rustix::io::Errno::NOSPC.raw_os_error())
    );

    let open_unlinked = File::open(&object_path).unwrap();
    std::fs::remove_file(&object_path).unwrap();
    std::fs::remove_file(object_limited.path().join("alias")).unwrap();
    assert_eq!(
        object_limited
            .usage()
            .await
            .unwrap()
            .unwrap()
            .filesystem_objects,
        2
    );
    drop(open_unlinked);
    object_limited.close_and_delete().await.unwrap();
    assert_eq!(
        object_backend.usage(object_project).unwrap(),
        AgentFilesystemUsage {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );

    let deferred = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let deferred_project = match &deferred.storage {
        AgentFilesystemStorage::Managed { project_id, .. } => *project_id,
        AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
    };
    let retained_root = File::open(deferred.path()).unwrap();
    drop(deferred);
    drop(retained_root);
    let mut released = false;
    for _ in 0..500 {
        if backend.usage(deferred_project).unwrap()
            == (AgentFilesystemUsage {
                allocated_bytes: 0,
                filesystem_objects: 0,
            })
        {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(released, "deferred managed project cleanup did not finish");
}

#[cfg(unix)]
#[test]
async fn unmanaged_materialization_creates_distinct_owned_files() {
    use std::os::unix::fs::MetadataExt;

    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"shared initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id,
            initial_files: vec![
                initial_file(
                    content_hash,
                    "/first/immutable",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/second/immutable",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
            file_loader,
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();

    let first = filesystem.path().join("first/immutable");
    let second = filesystem.path().join("second/immutable");
    let writable = filesystem.path().join("writable");
    assert_eq!(std::fs::read(&first).unwrap(), content);
    assert_eq!(std::fs::read(&second).unwrap(), content);
    assert_eq!(std::fs::read(&writable).unwrap(), content);
    assert_ne!(
        first.metadata().unwrap().ino(),
        second.metadata().unwrap().ino()
    );
    assert_ne!(
        first.metadata().unwrap().ino(),
        writable.metadata().unwrap().ino()
    );
    assert!(filesystem.runtime().is_read_only(&first));
    assert!(filesystem.runtime().is_read_only(&second));
    assert!(!filesystem.runtime().is_read_only(&writable));
    assert!(
        filesystem
            .runtime()
            .is_read_only(&filesystem.path().join("first/../first/immutable"))
    );
    std::os::unix::fs::symlink(&first, filesystem.path().join("immutable-link")).unwrap();
    assert!(
        filesystem
            .runtime()
            .is_read_only(&filesystem.path().join("immutable-link"))
    );
    assert!(
        !filesystem
            .runtime()
            .is_read_only_path(&filesystem.path().join("immutable-link"), false,)
    );
    tokio::fs::write(&writable, b"changed").await.unwrap();

    let path = filesystem.path().to_path_buf();
    filesystem.close_and_delete().await.unwrap();
    assert!(!path.exists());
}

#[test]
async fn failed_initial_file_update_preserves_current_files() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: vec![initial_file(
                content_hash,
                "/current",
                AgentFilePermissions::ReadOnly,
                content.len() as u64,
            )],
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();

    let result = filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[
                initial_file(
                    content_hash,
                    "/new",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/invalid",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64 + 1,
                ),
            ],
        )
        .await;

    assert!(result.is_err());
    let current = filesystem.path().join("current");
    assert_eq!(std::fs::read(&current).unwrap(), content);
    assert!(filesystem.runtime().is_read_only(&current));
    assert!(!filesystem.path().join("new").exists());
    assert!(!filesystem.path().join("invalid").exists());
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn initial_file_update_commits_staged_files_and_policy_together() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: vec![initial_file(
                content_hash,
                "/old",
                AgentFilePermissions::ReadOnly,
                content.len() as u64,
            )],
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();

    filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[
                initial_file(
                    content_hash,
                    "/new",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
        )
        .await
        .unwrap();

    let new = filesystem.path().join("new");
    let writable = filesystem.path().join("writable");
    assert!(!filesystem.path().join("old").exists());
    assert_eq!(std::fs::read(&new).unwrap(), content);
    assert_eq!(std::fs::read(&writable).unwrap(), content);
    assert!(filesystem.runtime().is_read_only(&new));
    assert!(!filesystem.runtime().is_read_only(&writable));
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn initial_file_updates_are_exclusive_with_filesystem_effects() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let effect = runtime.begin_effect().await.unwrap();
    let update_runtime = runtime.clone();
    let update = tokio::spawn(async move { update_runtime.begin_update_effect().await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!update.is_finished());

    drop(effect);
    let update = update.await.unwrap();
    let effect_runtime = runtime.clone();
    let next_effect = tokio::spawn(async move { effect_runtime.begin_effect().await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!next_effect.is_finished());

    drop(update);
    drop(next_effect.await.unwrap());
}

#[test]
fn dropped_initial_file_transaction_restores_backups() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let staged = root.path().join("staged");
    let backup = root.path().join("backups");
    std::fs::write(&live, b"old").unwrap();
    std::fs::write(&staged, b"new").unwrap();
    std::fs::create_dir(&backup).unwrap();

    {
        let mut transaction = InitialFileUpdateTransaction::new(backup);
        transaction.back_up(&live).unwrap();
        transaction.install(&staged, &live).unwrap();
    }

    assert_eq!(std::fs::read(&live).unwrap(), b"old");
}

#[test]
async fn initial_file_update_rejects_guest_file_collision() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: Vec::new(),
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();
    let collision = filesystem.path().join("collision");
    std::fs::write(&collision, b"guest data").unwrap();

    let result = filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[initial_file(
                content_hash,
                "/collision",
                AgentFilePermissions::ReadOnly,
                content.len() as u64,
            )],
        )
        .await;

    assert!(result.is_err());
    assert_eq!(std::fs::read(collision).unwrap(), b"guest data");
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn initial_file_update_preserves_guest_file_for_read_write_target() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: Vec::new(),
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();
    let collision = filesystem.path().join("collision");
    std::fs::write(&collision, b"guest data").unwrap();

    let update = filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[initial_file(
                content_hash,
                "/collision",
                AgentFilePermissions::ReadWrite,
                content.len() as u64,
            )],
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(collision).unwrap(), b"guest data");
    drop(update);
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn deterministic_creation_removes_existing_garbage() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();

    let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
    assert_eq!(filesystem.usage().await.unwrap(), None);
    let path = filesystem.path().to_path_buf();
    tokio::fs::write(path.join("garbage"), b"old")
        .await
        .unwrap();
    drop(filesystem);
    tokio::fs::create_dir_all(&path).await.unwrap();
    tokio::fs::write(path.join("garbage"), b"old")
        .await
        .unwrap();

    let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
    assert!(!filesystem.path().join("garbage").exists());
    filesystem.close_and_delete().await.unwrap();
    assert!(!path.exists());
}

#[test]
async fn seal_rejects_new_effects_without_waiting_for_existing_effects() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let runtime = filesystem.runtime();
    let effect = runtime.begin_effect().await.unwrap();

    filesystem.seal();
    assert!(runtime.begin_effect().await.is_err());
    assert!(filesystem.path().exists());
    drop(effect);
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn close_waits_for_an_existing_effect_before_deleting() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let path = filesystem.path().to_path_buf();
    let effect = filesystem.runtime().begin_effect().await.unwrap();

    let close = tokio::spawn(filesystem.close_and_delete());
    tokio::task::yield_now().await;
    assert!(!close.is_finished());
    assert!(path.exists());
    drop(effect);
    close.await.unwrap().unwrap();
    assert!(!path.exists());
}

#[test]
async fn reconstruction_settlement_waits_for_existing_effects() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let effect = filesystem.runtime().begin_effect().await.unwrap();
    {
        let settle = filesystem.settle_reconstruction();
        tokio::pin!(settle);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut settle)
                .await
                .is_err()
        );
        drop(effect);
        settle.await.unwrap();
    }
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn dropped_owner_defers_cleanup_and_retains_lifecycle_until_effects_finish() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
    let path = filesystem.path().to_path_buf();
    let effect = filesystem.runtime().begin_effect().await.unwrap();
    drop(filesystem);

    let replacement = tokio::spawn({
        let filesystems = filesystems.clone();
        let id = id.clone();
        async move { filesystems.create_owned_empty(&id).await }
    });
    tokio::task::yield_now().await;
    assert!(!replacement.is_finished());
    assert!(path.exists());

    drop(effect);
    let replacement = tokio::time::timeout(std::time::Duration::from_secs(5), replacement)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    replacement.close_and_delete().await.unwrap();
}

#[test]
async fn deterministic_creation_is_exclusive_for_the_full_owner_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let first = filesystems.create_owned_empty(&id).await.unwrap();
    tokio::fs::write(first.path().join("owned"), b"first")
        .await
        .unwrap();

    let second = tokio::spawn({
        let filesystems = filesystems.clone();
        let id = id.clone();
        async move { filesystems.create_owned_empty(&id).await }
    });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    assert!(first.path().join("owned").exists());

    first.close_and_delete().await.unwrap();
    let second = second.await.unwrap().unwrap();
    assert!(!second.path().join("owned").exists());
    second.close_and_delete().await.unwrap();
}

#[test]
async fn append_effect_ends_at_native_completion_without_waiting_for_guest_polling() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let ready = Arc::new(tokio::sync::Semaphore::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
        ready: Arc::clone(&ready),
        cancelled,
    }));
    stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
    stream.write(Bytes::from_static(b"first")).unwrap();

    assert!(stream.is_active());
    assert!(stream.write(Bytes::from_static(b"second")).is_err());
    let next_effect = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.begin_append_effect().await }
    });
    tokio::task::yield_now().await;
    assert!(!next_effect.is_finished());

    ready.add_permits(1);
    let next_effect = next_effect.await.unwrap().unwrap();
    assert!(!stream.is_active());
    drop(next_effect);
}

#[test]
async fn positioned_effect_does_not_wait_for_active_append() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let append = runtime.begin_append_effect().await.unwrap();

    let positioned = runtime.begin_effect().await.unwrap();

    drop(positioned);
    drop(append);
}

#[test]
async fn cancelling_p2_stream_forwards_cancellation_and_releases_the_effect() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
        ready: Arc::new(tokio::sync::Semaphore::new(0)),
        cancelled: Arc::clone(&cancelled),
    }));
    stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
    stream.write(Bytes::from_static(b"write")).unwrap();

    let next_append = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.begin_append_effect().await }
    });
    tokio::task::yield_now().await;
    assert!(!next_append.is_finished());

    stream.cancel().await;

    assert!(cancelled.load(Ordering::Acquire));
    assert!(!stream.is_active());
    assert!(next_append.await.unwrap().is_ok());
}

#[test]
async fn dropping_p2_stream_requests_cancellation() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut stream = CoordinatedFileOutputStream::new(Box::new(DelayedOutputStream {
        ready: Arc::new(tokio::sync::Semaphore::new(0)),
        cancelled: Arc::clone(&cancelled),
    }));
    stream.prepare_effect(runtime.begin_append_effect().await.unwrap());
    stream.write(Bytes::from_static(b"write")).unwrap();

    drop(stream);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !cancelled.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stream drop did not request cancellation");
    assert!(runtime.begin_append_effect().await.is_ok());
}
