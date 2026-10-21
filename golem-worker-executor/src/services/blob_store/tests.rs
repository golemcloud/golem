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

use crate::durable_host::blobstore::{
    BlobProviderOperation, await_blob_provider, classify_blob_store_error,
};
use crate::durable_host::durability::HostFailureKind;
use crate::services::blob_store::{
    BlobMutationFailure, BlobMutationFailureCause, BlobStoreError, BlobStoreMutation,
    BlobStoreService, DefaultBlobStoreService, blob_store_error,
};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::worker::entity_invocation::{RetainedEntityStore, await_component_entity_body};
use crate::worker::entity_slot::{EntitySlot, EntitySlotRegistration};
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::agent::Principal;
use golem_common::model::component::ComponentId;
use golem_common::model::entity::{
    EntityActivation, EntityActivationPolicy, EntityInvocationId, EntityInvocationScope,
    ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnedAgentEntityId,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::{SecretKeyScope, ToolFilesystemAccess};
use golem_common::model::{AgentId, IdempotencyKey, OplogIndex, OwnedAgentId};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobNameError, BlobRangeError, BlobRangeStream, BlobStorage,
    BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath,
    PutIfAbsent,
};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::TempDir;
use test_r::test;

fn unlimited_limits() -> Arc<AtomicResourceEntry> {
    Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
    ))
}

/// A blob storage that delegates to an in-memory storage, and that can fail or hold its next
/// `put_raw` or copy, and observes actual backend return versus future drop.
#[derive(Debug)]
struct FailingPutBlobStorage {
    inner: InMemoryBlobStorage,
    fail_next_put: AtomicBool,
    block_next_put: AtomicBool,
    fail_next_copy: AtomicBool,
    block_next_copy: AtomicBool,
    mutations_returned: AtomicUsize,
    mutations_failed: AtomicUsize,
    mutations_dropped: AtomicUsize,
    nameless_listing: bool,
    put_started: tokio::sync::Notify,
    release_put: tokio::sync::Notify,
}

impl FailingPutBlobStorage {
    fn new() -> Self {
        Self {
            inner: InMemoryBlobStorage::new(),
            fail_next_put: AtomicBool::new(false),
            block_next_put: AtomicBool::new(false),
            fail_next_copy: AtomicBool::new(false),
            block_next_copy: AtomicBool::new(false),
            mutations_returned: AtomicUsize::new(0),
            mutations_failed: AtomicUsize::new(0),
            mutations_dropped: AtomicUsize::new(0),
            nameless_listing: false,
            put_started: tokio::sync::Notify::new(),
            release_put: tokio::sync::Notify::new(),
        }
    }

    fn fail_next_put(&self) {
        self.fail_next_put.store(true, Ordering::Release);
    }

    fn block_next_put(&self) {
        self.block_next_put.store(true, Ordering::Release);
    }

    async fn wait_for_blocked_put(&self) {
        self.put_started.notified().await;
    }

    fn release_blocked_put(&self) {
        self.release_put.notify_one();
    }

    async fn begin_mutation(&self, block: &AtomicBool) -> Option<BackendMutationObservation<'_>> {
        if !block.swap(false, Ordering::AcqRel) {
            return None;
        }
        let observation = BackendMutationObservation {
            storage: self,
            returned: false,
        };
        self.put_started.notify_one();
        self.release_put.notified().await;
        Some(observation)
    }

    fn finish_mutation(&self, observation: Option<BackendMutationObservation<'_>>, failed: bool) {
        if let Some(mut observation) = observation {
            observation.returned = true;
            self.mutations_returned.fetch_add(1, Ordering::SeqCst);
            if failed {
                self.mutations_failed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

struct BackendMutationObservation<'a> {
    storage: &'a FailingPutBlobStorage,
    returned: bool,
}

impl Drop for BackendMutationObservation<'_> {
    fn drop(&mut self) {
        if !self.returned {
            self.storage
                .mutations_dropped
                .fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl BlobStorageBackend for FailingPutBlobStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.inner
            .get_raw_at(target_label, op_label, namespace, path)
            .await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        self.inner
            .get_range_stream_at(target_label, op_label, namespace, path, offset, length)
            .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
        self.inner
            .get_metadata_at(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        let observation = self.begin_mutation(&self.block_next_put).await;
        let result = if self.fail_next_put.swap(false, Ordering::AcqRel) {
            Err(anyhow::anyhow!("injected put failure"))
        } else {
            self.inner
                .put_raw_at(target_label, op_label, namespace, path, data)
                .await
        };
        self.finish_mutation(observation, result.is_err());
        result
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        self.inner
            .put_raw_if_absent_at(target_label, op_label, namespace, path, data)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        self.inner
            .delete_at(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        self.inner
            .create_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        if self.nameless_listing {
            return Ok(vec![PathBuf::new()]);
        }
        self.inner
            .list_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        self.inner
            .list_blobs_below_at(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<u64>, Error> {
        self.inner
            .delete_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
        self.inner
            .exists_at(target_label, op_label, namespace, path)
            .await
    }

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        let observation = self.begin_mutation(&self.block_next_copy).await;
        let result = if self.fail_next_copy.swap(false, Ordering::AcqRel) {
            Err(anyhow::anyhow!("injected copy failure"))
        } else {
            self.inner
                .copy_between_at(
                    target_label,
                    op_label,
                    from_namespace,
                    from,
                    to_namespace,
                    to,
                )
                .await
        };
        self.finish_mutation(observation, result.is_err());
        result
    }
}

/// A read that the storage refuses, here because the executor cannot search the blob root,
/// is not an error of the input of the guest and not one that a retry can pass. The guest
/// gets a permanent `PermissionDenied` that names the refusal.
#[cfg(unix)]
#[test]
async fn blob_store_error_makes_a_refusal_of_the_storage_a_permanent_permission_error() {
    use std::os::unix::fs::PermissionsExt;
    let tempdir = TempDir::new().unwrap();
    let storage = FileSystemBlobStorage::new(tempdir.path()).await.unwrap();
    let namespace = BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId::new(),
    };
    let path = Path::new("container/object");
    storage
        .put_raw("test", "put-raw", namespace.clone(), path, &[1])
        .await
        .unwrap();
    let set_mode = |mode| {
        std::fs::set_permissions(tempdir.path(), std::fs::Permissions::from_mode(mode)).unwrap()
    };

    set_mode(0o000);
    let read = storage.get_raw("test", "get-raw", namespace, path).await;
    set_mode(0o755);
    let error = blob_store_error(read.unwrap_err());

    assert_eq!(
        (
            matches!(&error, BlobStoreError::PermissionDenied(message)
                    if message.starts_with("the blob storage refused the request (access denied)")),
            classify_blob_store_error(&error)
        ),
        (true, HostFailureKind::Permanent),
        "{error}"
    );
}

/// `BlobFailure::of` has one downcast for `BlobNameError`, so each rule of a name is
/// permanent, and the rules of the path are in it with the rules of the object key of S3.
///
/// The filesystem backend gives the three errors of the path, so the test reads the real
/// rules and breaks if the type of their error changes. The NUL rule is a rule of MinIO,
/// and every backend applies it, so the filesystem backend gives it here too. The backend
/// also gives an error of its own: a write of a path that is too long for the filesystem.
#[test]
async fn blob_store_error_makes_an_error_of_the_input_permanent_and_a_backend_error_transient() {
    let tempdir = TempDir::new().unwrap();
    let storage = FileSystemBlobStorage::new(tempdir.path()).await.unwrap();
    let namespace = BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId::new(),
    };
    let put = |path: String| {
        let namespace = namespace.clone();
        let storage = &storage;
        async move {
            storage
                .put_raw("test", "put-raw", namespace, Path::new(&path), &[1])
                .await
        }
    };

    // The backend writes a name of 3000 bytes as 6000 hex characters, so the path on disk
    // is longer than the filesystem accepts.
    let backend = blob_store_error(put("x".repeat(3000)).await.unwrap_err());
    let names = [
        put("../escape".to_string()).await.unwrap_err(),
        put("/escape".to_string()).await.unwrap_err(),
        put("a\0b".to_string()).await.unwrap_err(),
        BlobRangeError { start: 3, end: 2 }.into(),
    ]
    .map(blob_store_error);

    assert_eq!(
        names
            .iter()
            .map(|error| (error.to_string(), classify_blob_store_error(error)))
            .collect::<Vec<_>>(),
        vec![
            (
                format!(
                    "Invalid input: {}",
                    BlobNameError::ParentDir {
                        path: PathBuf::from("../escape")
                    }
                ),
                HostFailureKind::Permanent
            ),
            (
                format!(
                    "Invalid input: {}",
                    BlobNameError::NotRelative {
                        path: PathBuf::from("/escape")
                    }
                ),
                HostFailureKind::Permanent
            ),
            (
                format!("Invalid input: {}", BlobNameError::NulByte),
                HostFailureKind::Permanent
            ),
            (
                format!("Invalid input: {}", BlobRangeError { start: 3, end: 2 }),
                HostFailureKind::Permanent
            ),
        ]
    );
    assert_eq!(
        (
            matches!(backend, BlobStoreError::TransientBackend(_)),
            classify_blob_store_error(&backend)
        ),
        (true, HostFailureKind::Transient),
        "{backend:?}"
    );
}

/// The container name has a NUL byte, which breaks a rule of a name
/// (`BlobNameError::NulByte`), and each operation puts the container name in the path that
/// it gives to the storage. The storage applies the rules of a name to every path before a
/// backend gets it, so each operation gets the error of the name.
#[test]
async fn every_operation_gives_invalid_input_for_a_name_error() {
    let blob_store = DefaultBlobStoreService::new(Arc::new(InMemoryBlobStorage::new()));
    let environment_id = EnvironmentId::new();
    let container = || "cont\0ainer".to_string();
    let object = || "object".to_string();

    let errors: Vec<Result<(), BlobStoreError>> = vec![
        blob_store
            .clear(unlimited_limits(), environment_id, container())
            .await
            .map(drop),
        blob_store
            .container_exists(environment_id, container())
            .await
            .map(drop),
        blob_store
            .copy_object(
                unlimited_limits(),
                environment_id,
                container(),
                object(),
                container(),
                object(),
            )
            .await
            .map(drop),
        blob_store
            .create_container(environment_id, container())
            .await
            .map(drop),
        blob_store
            .delete_container(unlimited_limits(), environment_id, container())
            .await
            .map(drop),
        blob_store
            .delete_object(unlimited_limits(), environment_id, container(), object())
            .await
            .map(drop),
        blob_store
            .delete_objects(
                unlimited_limits(),
                environment_id,
                &container(),
                &[object()],
            )
            .await
            .map(drop),
        blob_store
            .get_container(environment_id, container())
            .await
            .map(drop),
        blob_store
            .get_data(environment_id, container(), object(), 0, 1)
            .await
            .map(drop),
        blob_store
            .has_object(environment_id, container(), object())
            .await
            .map(drop),
        blob_store
            .list_objects(environment_id, container())
            .await
            .map(drop),
        blob_store
            .move_object(
                unlimited_limits(),
                environment_id,
                container(),
                object(),
                container(),
                object(),
            )
            .await
            .map(drop),
        blob_store
            .object_info(environment_id, container(), object())
            .await
            .map(drop),
        blob_store
            .write_data(
                unlimited_limits(),
                environment_id,
                &container(),
                "object",
                &[1],
            )
            .await
            .map(drop),
    ];

    assert_eq!(
        errors
            .iter()
            .map(|result| match result {
                Err(error @ BlobStoreError::InvalidInput(_)) => {
                    classify_blob_store_error(error) == HostFailureKind::Permanent
                }
                _ => false,
            })
            .collect::<Vec<_>>(),
        vec![true; 14],
        "{errors:?}"
    );
}

async fn test_container_exists(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    assert!(
        !blob_store
            .container_exists(environment_id, "container1".to_string())
            .await
            .unwrap()
    );
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();
    assert!(
        blob_store
            .container_exists(environment_id, "container1".to_string())
            .await
            .unwrap()
    );
}

async fn test_container_delete(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();
    blob_store
        .delete_container(unlimited_limits(), environment_id, "container1".to_string())
        .await
        .unwrap();
    assert!(
        !blob_store
            .container_exists(environment_id, "container1".to_string())
            .await
            .unwrap()
    );
}

async fn test_container_has_write_read_has(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();

    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();
    assert!(
        !blob_store
            .has_object(environment_id, "container1".to_string(), "obj1".to_string())
            .await
            .unwrap()
    );

    let original_data = vec![1, 2, 3, 4];
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container1",
            "obj1",
            &original_data,
        )
        .await
        .unwrap();

    let read_data = blob_store
        .get_data(
            environment_id,
            "container1".to_string(),
            "obj1".to_string(),
            0,
            3,
        )
        .await
        .unwrap();

    assert_eq!(original_data, read_data);
    assert!(
        blob_store
            .has_object(environment_id, "container1".to_string(), "obj1".to_string())
            .await
            .unwrap()
    );
}

async fn test_container_list_copy_move_list(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();

    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();
    blob_store
        .create_container(environment_id, "container2".to_string())
        .await
        .unwrap();

    assert!(
        blob_store
            .list_objects(environment_id, "container1".to_string(),)
            .await
            .unwrap()
            .is_empty()
    );

    let original_data = vec![1, 2, 3, 4];
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container1",
            "obj1",
            &original_data,
        )
        .await
        .unwrap();

    blob_store
        .copy_object(
            unlimited_limits(),
            environment_id,
            "container1".to_string(),
            "obj1".to_string(),
            "container1".to_string(),
            "obj2".to_string(),
        )
        .await
        .unwrap();

    let mut result = blob_store
        .list_objects(environment_id, "container1".to_string())
        .await
        .unwrap();

    result.sort();

    assert_eq!(result, vec!["obj1", "obj2"]);

    blob_store
        .move_object(
            unlimited_limits(),
            environment_id,
            "container1".to_string(),
            "obj1".to_string(),
            "container2".to_string(),
            "obj3".to_string(),
        )
        .await
        .unwrap();

    assert_eq!(
        blob_store
            .list_objects(environment_id, "container1".to_string(),)
            .await
            .unwrap(),
        vec!["obj2"]
    );

    assert_eq!(
        blob_store
            .list_objects(environment_id, "container2".to_string(),)
            .await
            .unwrap(),
        vec!["obj3"]
    );
}

async fn test_get_data_outside_the_object_is_invalid_input(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container1",
            "obj1",
            &[1, 2, 3, 4],
        )
        .await
        .unwrap();
    let read = |start, end| {
        blob_store.get_data(
            environment_id,
            "container1".to_string(),
            "obj1".to_string(),
            start,
            end,
        )
    };

    assert_eq!(read(1, 2).await.unwrap(), vec![2, 3]);

    let outside =
        futures::future::join_all([(0, 4), (4, 4), (2, 1)].map(|(start, end)| read(start, end)))
            .await;
    assert!(
        outside.iter().all(|result| matches!(
            result,
            Err(error @ BlobStoreError::InvalidInput(_))
                if classify_blob_store_error(error) == HostFailureKind::Permanent
        )),
        "{outside:?}"
    );
}

/// A guest picks the name of a container and the name of an object, and a `..` in a name
/// makes a path that goes above the root of its namespace. Each backend rejects such a
/// path, and the guest gets a permanent error, so the executor does not retry a name that
/// can never work.
async fn test_a_parent_name_is_invalid_input(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();

    let written = blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container1",
            "../../escape",
            &[1],
        )
        .await
        .map(drop);
    let read = blob_store
        .get_data(
            environment_id,
            "container1".to_string(),
            "../../escape".to_string(),
            0,
            0,
        )
        .await
        .map(drop);
    let container = blob_store
        .create_container(environment_id, "../escape".to_string())
        .await
        .map(drop);

    let results = [written, read, container];
    assert!(
        results.iter().all(|result| matches!(
            result,
            Err(error @ BlobStoreError::InvalidInput(_))
                if classify_blob_store_error(error) == HostFailureKind::Permanent
        )),
        "{results:?}"
    );
}

/// A guest picks the container name, and `""`, `"."`, `"./"` and `"././"` are four spellings
/// of one name: the root of the namespace, because a `.` is not a name. The root is the
/// namespace itself and not a container, so every method that takes a container name gives
/// `BlobStoreError::InvalidInput` for each of the four
/// (`DefaultBlobStoreService::container_path`). The error is permanent, so the guest gets it
/// on the first call and the executor does not retry a name that can never name a container.
///
/// The rule sits above the backends, so both backends here give it and so do the other two.
/// Each backend keeps its own rule about a root path, which
/// `golem_service_base::storage::blob` states and `golem-worker-service/tests/blob_storage.rs`
/// holds: a root path is a directory, so a read there finds no blob, a delete there removes
/// none, and `create_dir` and `delete_dir` there change nothing.
///
/// `Host::create_container` reads the container back after it makes one, for the time that
/// the guest gets (`crate::durable_host::blobstore`). The rule is what keeps that read from
/// asking about a directory that `create_dir` never made.
async fn test_a_root_container_name_is_invalid_input(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();

    for root_name in ["", ".", "./", "././"] {
        let root = || root_name.to_string();
        let object = || "object".to_string();

        let results: Vec<Result<(), BlobStoreError>> = vec![
            blob_store
                .clear(unlimited_limits(), environment_id, root())
                .await
                .map(drop),
            blob_store
                .container_exists(environment_id, root())
                .await
                .map(drop),
            blob_store
                .copy_object(
                    unlimited_limits(),
                    environment_id,
                    root(),
                    object(),
                    "container1".into(),
                    object(),
                )
                .await
                .map(drop),
            blob_store
                .copy_object(
                    unlimited_limits(),
                    environment_id,
                    "container1".into(),
                    object(),
                    root(),
                    object(),
                )
                .await
                .map(drop),
            blob_store
                .create_container(environment_id, root())
                .await
                .map(drop),
            blob_store
                .delete_container(unlimited_limits(), environment_id, root())
                .await
                .map(drop),
            blob_store
                .delete_object(unlimited_limits(), environment_id, root(), object())
                .await
                .map(drop),
            blob_store
                .delete_objects(unlimited_limits(), environment_id, root_name, &[object()])
                .await
                .map(drop),
            blob_store
                .get_container(environment_id, root())
                .await
                .map(drop),
            blob_store
                .get_data(environment_id, root(), object(), 0, 0)
                .await
                .map(drop),
            blob_store
                .has_object(environment_id, root(), object())
                .await
                .map(drop),
            blob_store
                .list_objects(environment_id, root())
                .await
                .map(drop),
            blob_store
                .move_object(
                    unlimited_limits(),
                    environment_id,
                    root(),
                    object(),
                    "container1".into(),
                    object(),
                )
                .await
                .map(drop),
            blob_store
                .move_object(
                    unlimited_limits(),
                    environment_id,
                    "container1".into(),
                    object(),
                    root(),
                    object(),
                )
                .await
                .map(drop),
            blob_store
                .object_info(environment_id, root(), object())
                .await
                .map(drop),
            blob_store
                .write_data(
                    unlimited_limits(),
                    environment_id,
                    root_name,
                    "object",
                    &[1],
                )
                .await
                .map(drop),
        ];

        assert!(
            results.iter().all(|result| matches!(
                result,
                Err(error @ BlobStoreError::InvalidInput(_))
                    if classify_blob_store_error(error) == HostFailureKind::Permanent
            )),
            "the container name {root_name:?} gave {results:?}"
        );
    }

    assert_eq!(
        blob_store
            .list_objects(environment_id, "container1".to_string())
            .await
            .unwrap(),
        Vec::<String>::new(),
        "a root container name wrote an object"
    );
}

/// A guest picks the source container name and the source object name of `copy_object` and
/// of `move_object`, and a source object that is not there gives a permanent
/// `BlobStoreError::NotFound`. The name is good, so it is not an error of the name: the
/// storage holds no object at it, and a retry cannot make the storage hold it.
///
/// The error names the path of the source object, and the copy writes nothing. The test
/// does not hold that the move deletes nothing: the delete of a move is of the source, and
/// the source is not there, so the listing is the same whether the delete runs or not. The
/// S3 test `move_gives_a_missing_error_for_a_source_that_is_not_there_and_deletes_nothing`
/// holds that, by the one request that the move sends.
async fn test_a_source_that_is_not_there_is_not_found(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container1",
            "obj1",
            &[1],
        )
        .await
        .unwrap();

    let copied = blob_store
        .copy_object(
            unlimited_limits(),
            environment_id,
            "container1".to_string(),
            "missing".to_string(),
            "container1".to_string(),
            "obj2".to_string(),
        )
        .await;
    let moved = blob_store
        .move_object(
            unlimited_limits(),
            environment_id,
            "container1".to_string(),
            "missing".to_string(),
            "container1".to_string(),
            "obj3".to_string(),
        )
        .await;

    let expected = format!(
        "Not found: {}",
        BlobMissingError {
            path: PathBuf::from("container1/missing"),
        }
    );
    assert_eq!(
        [copied, moved].map(|result| result
            .err()
            .map(|error| (error.to_string(), classify_blob_store_error(&error)))),
        [
            Some((expected.clone(), HostFailureKind::Permanent)),
            Some((expected, HostFailureKind::Permanent)),
        ]
    );
    assert_eq!(
        blob_store
            .list_objects(environment_id, "container1".to_string())
            .await
            .unwrap(),
        vec!["obj1"],
        "a source that is not there writes nothing"
    );
}

/// A guest picks the source container name and the source object name, so the guest writes
/// the path of the source. `./missing` and `missing` are two forms of one path, and the
/// storage normalizes the path before a backend reads it. The error names the path as the
/// guest wrote it, as a `BlobNameError` does, because the guest reads the message and the
/// normalized form is of the storage.
///
/// The copy is onto the same path, so the storage reads the source and writes nothing. The
/// in-memory and the filesystem backends read it with the default `has_blob_at` of
/// `BlobStorageBackend`. The S3 backend reads it with one `HeadObject`, which
/// `copy_names_the_source_path_as_the_guest_wrote_it` in
/// `golem_service_base::storage::blob::s3::tests` holds.
async fn test_a_missing_source_names_the_path_that_the_guest_wrote(
    blob_store: &impl BlobStoreService,
) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container1".to_string())
        .await
        .unwrap();

    let copied = blob_store
        .copy_object(
            unlimited_limits(),
            environment_id,
            "container1".to_string(),
            "./missing".to_string(),
            "container1".to_string(),
            "missing".to_string(),
        )
        .await;

    assert_eq!(
        copied.map_err(|error| error.to_string()),
        Err(format!(
            "Not found: {}",
            BlobMissingError {
                path: PathBuf::from("container1/./missing"),
            }
        ))
    );
}

async fn test_container_name_spellings(blob_store: &impl BlobStoreService) {
    for container_name in ["c/", "./c"] {
        let environment_id = EnvironmentId::new();
        blob_store
            .write_data(
                unlimited_limits(),
                environment_id,
                container_name,
                "x",
                b"data",
            )
            .await
            .unwrap();
        assert_eq!(
            blob_store
                .list_objects(environment_id, container_name.to_string())
                .await
                .unwrap(),
            vec!["x"]
        );
    }
}

async fn test_contract_object_name(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    let object_name = r"animals\cat.png";
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "photos",
            object_name,
            b"data",
        )
        .await
        .unwrap();
    assert!(
        blob_store
            .has_object(
                environment_id,
                "photos".to_string(),
                object_name.to_string()
            )
            .await
            .unwrap()
    );
    assert_eq!(
        blob_store
            .list_objects(environment_id, "photos".to_string())
            .await
            .unwrap(),
        vec![object_name]
    );
}

async fn test_absolute_object_name_is_invalid(blob_store: &impl BlobStoreService) {
    let error = blob_store
        .write_data(
            unlimited_limits(),
            EnvironmentId::new(),
            "photos",
            "/cat.png",
            b"data",
        )
        .await
        .unwrap_err();
    assert!(matches!(error, BlobStoreError::InvalidInput(_)));
}

async fn test_invalid_container_name_is_permanent(blob_store: &impl BlobStoreService) {
    for container_name in ["/photos", "../photos"] {
        let environment_id = EnvironmentId::new();
        let error = blob_store
            .create_container(environment_id, container_name.to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, BlobStoreError::InvalidInput(_)));

        let error = blob_store
            .delete_objects(unlimited_limits(), environment_id, container_name, &[])
            .await
            .unwrap_err();
        assert!(matches!(error, BlobStoreError::InvalidInput(_)));
    }
}

fn in_memory_blob_store() -> DefaultBlobStoreService {
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    DefaultBlobStoreService::new(blob_storage)
}

async fn fs_blob_store(path: &Path) -> impl BlobStoreService {
    let blob_storage = Arc::new(FileSystemBlobStorage::new(path).await.unwrap());
    DefaultBlobStoreService::new(blob_storage)
}

async fn sqlite_blob_store() -> impl BlobStoreService {
    let sqlx_pool = SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let pool = SqlitePool::new(sqlx_pool.clone(), sqlx_pool);
    DefaultBlobStoreService::new(Arc::new(SqliteBlobStorage::new(pool).await.unwrap()))
}

#[test]
async fn list_objects_rejects_a_backend_path_without_a_name() {
    let mut storage = FailingPutBlobStorage::new();
    storage.nameless_listing = true;
    let blob_store = DefaultBlobStoreService::new(Arc::new(storage));
    let result = blob_store
        .list_objects(EnvironmentId::new(), "container".to_string())
        .await;
    assert!(matches!(result, Err(BlobStoreError::InvalidInput(_))));
}

#[test]
fn object_path_rejects_an_absolute_object_name() {
    let result = DefaultBlobStoreService::object_path("container", "/object");
    assert!(matches!(result, Err(BlobStoreError::InvalidInput(_))));
}

async fn test_root_object_is_invalid(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container",
            "object",
            b"kept",
        )
        .await
        .unwrap();
    for root in ["", ".", "./", "././"] {
        let errors = [
            blob_store
                .write_data(
                    unlimited_limits(),
                    environment_id,
                    "container",
                    root,
                    b"data",
                )
                .await
                .map(drop),
            blob_store
                .delete_object(
                    unlimited_limits(),
                    environment_id,
                    "container".to_string(),
                    root.to_string(),
                )
                .await
                .map(drop),
            blob_store
                .delete_objects(
                    unlimited_limits(),
                    environment_id,
                    "container",
                    &["object".to_string(), root.to_string()],
                )
                .await
                .map(drop),
            blob_store
                .has_object(environment_id, "container".to_string(), root.to_string())
                .await
                .map(drop),
            blob_store
                .object_info(environment_id, "container".to_string(), root.to_string())
                .await
                .map(drop),
            blob_store
                .copy_object(
                    unlimited_limits(),
                    environment_id,
                    "container".to_string(),
                    root.to_string(),
                    "container".to_string(),
                    "object".to_string(),
                )
                .await
                .map(drop),
            blob_store
                .copy_object(
                    unlimited_limits(),
                    environment_id,
                    "container".to_string(),
                    "object".to_string(),
                    "container".to_string(),
                    root.to_string(),
                )
                .await
                .map(drop),
            blob_store
                .move_object(
                    unlimited_limits(),
                    environment_id,
                    "container".to_string(),
                    root.to_string(),
                    "container".to_string(),
                    "object".to_string(),
                )
                .await
                .map(drop),
            blob_store
                .move_object(
                    unlimited_limits(),
                    environment_id,
                    "container".to_string(),
                    "object".to_string(),
                    "container".to_string(),
                    root.to_string(),
                )
                .await
                .map(drop),
        ];
        assert!(
            errors
                .iter()
                .all(|result| matches!(result, Err(BlobStoreError::InvalidInput(_)))),
            "{errors:?}"
        );
        assert!(
            blob_store
                .container_exists(environment_id, "container".to_string())
                .await
                .unwrap()
        );
        assert_eq!(
            blob_store
                .get_data(
                    environment_id,
                    "container".to_string(),
                    "object".to_string(),
                    0,
                    3
                )
                .await
                .unwrap(),
            b"kept"
        );
        assert!(!matches!(
            blob_store
                .get_data(
                    environment_id,
                    "container".to_string(),
                    root.to_string(),
                    0,
                    0
                )
                .await,
            Err(BlobStoreError::InvalidInput(_))
        ));
    }
}

#[test]
async fn test_root_object_is_invalid_in_memory() {
    test_root_object_is_invalid(&in_memory_blob_store()).await;
}

#[test]
async fn test_root_object_is_invalid_local() {
    let tempdir = TempDir::new().unwrap();
    test_root_object_is_invalid(&fs_blob_store(tempdir.path()).await).await;
}

#[test]
async fn test_root_object_is_invalid_sqlite() {
    test_root_object_is_invalid(&sqlite_blob_store().await).await;
}

#[test]
async fn test_root_container_is_invalid_sqlite() {
    test_a_root_container_name_is_invalid_input(&sqlite_blob_store().await).await;
}

#[test]
async fn test_missing_copy_and_move_are_permanent_local() {
    let tempdir = TempDir::new().unwrap();
    test_a_source_that_is_not_there_is_not_found(&fs_blob_store(tempdir.path()).await).await;
}

#[test]
async fn test_container_exists_in_memory() {
    let blob_store = in_memory_blob_store();
    test_container_exists(&blob_store).await;
}

#[test]
async fn test_container_exists_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_container_exists(&blob_store).await;
}

#[test]
async fn test_container_delete_in_memory() {
    let blob_store = in_memory_blob_store();
    test_container_delete(&blob_store).await;
}

#[test]
async fn test_container_delete_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_container_delete(&blob_store).await;
}

#[test]
async fn test_container_has_write_read_has_in_memory() {
    let blob_store = in_memory_blob_store();
    test_container_has_write_read_has(&blob_store).await;
}

#[test]
async fn test_container_has_write_read_has_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_container_has_write_read_has(&blob_store).await;
}

#[test]
async fn test_container_list_copy_move_list_in_memory() {
    let blob_store = in_memory_blob_store();
    test_container_list_copy_move_list(&blob_store).await;
}

#[test]
async fn test_container_list_copy_move_list_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_container_list_copy_move_list(&blob_store).await;
}

#[test]
async fn test_a_parent_name_is_invalid_input_in_memory() {
    let blob_store = in_memory_blob_store();
    test_a_parent_name_is_invalid_input(&blob_store).await;
}

#[test]
async fn test_a_parent_name_is_invalid_input_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_a_parent_name_is_invalid_input(&blob_store).await;
}

#[test]
async fn test_a_source_that_is_not_there_is_not_found_in_memory() {
    let blob_store = in_memory_blob_store();
    test_a_source_that_is_not_there_is_not_found(&blob_store).await;
}

#[test]
async fn test_a_missing_source_names_the_path_that_the_guest_wrote_in_memory() {
    let blob_store = in_memory_blob_store();
    test_a_missing_source_names_the_path_that_the_guest_wrote(&blob_store).await;
}

#[test]
async fn test_a_missing_source_names_the_path_that_the_guest_wrote_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_a_missing_source_names_the_path_that_the_guest_wrote(&blob_store).await;
}

#[test]
async fn test_a_root_container_name_is_invalid_input_in_memory() {
    let blob_store = in_memory_blob_store();
    test_a_root_container_name_is_invalid_input(&blob_store).await;
}

#[test]
async fn test_a_root_container_name_is_invalid_input_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_a_root_container_name_is_invalid_input(&blob_store).await;
}

#[test]
async fn test_get_data_outside_the_object_is_invalid_input_in_memory() {
    let blob_store = in_memory_blob_store();
    test_get_data_outside_the_object_is_invalid_input(&blob_store).await;
}

#[test]
async fn test_get_data_outside_the_object_is_invalid_input_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_get_data_outside_the_object_is_invalid_input(&blob_store).await;
}

#[test]
async fn test_container_name_spellings_in_memory() {
    test_container_name_spellings(&in_memory_blob_store()).await;
}

#[test]
async fn test_container_name_spellings_local() {
    let tempdir = TempDir::new().unwrap();
    let blob_store = fs_blob_store(tempdir.path()).await;
    test_container_name_spellings(&blob_store).await;
}

#[test]
async fn test_contract_object_name_in_memory() {
    test_contract_object_name(&in_memory_blob_store()).await;
}

#[test]
async fn test_contract_object_name_local() {
    let tempdir = TempDir::new().unwrap();
    test_contract_object_name(&fs_blob_store(tempdir.path()).await).await;
}

#[test]
async fn test_absolute_object_name_is_invalid_in_memory() {
    test_absolute_object_name_is_invalid(&in_memory_blob_store()).await;
}

#[test]
async fn test_absolute_object_name_is_invalid_local() {
    let tempdir = TempDir::new().unwrap();
    test_absolute_object_name_is_invalid(&fs_blob_store(tempdir.path()).await).await;
}

#[test]
async fn test_invalid_container_name_is_permanent_in_memory() {
    test_invalid_container_name_is_permanent(&in_memory_blob_store()).await;
}

#[test]
async fn test_invalid_container_name_is_permanent_local() {
    let tempdir = TempDir::new().unwrap();
    test_invalid_container_name_is_permanent(&fs_blob_store(tempdir.path()).await).await;
}

#[test]
async fn custom_storage_quota_uses_net_mutation_deltas() {
    let blob_store = in_memory_blob_store();
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(5);
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();

    let created = blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[1, 2, 3, 4],
        )
        .await
        .unwrap();
    assert_eq!(created.bytes_delta, 4);

    let rejected = blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[0; 6],
        )
        .await;
    assert!(matches!(rejected, Err(BlobStoreError::LimitExceeded(_))));

    let replaced = blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[1, 2],
        )
        .await
        .unwrap();
    assert_eq!(replaced.bytes_delta, -2);

    let copied = blob_store
        .copy_object(
            limits.clone(),
            environment_id,
            "container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "destination".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(copied.bytes_delta, 2);

    let moved = blob_store
        .move_object(
            limits.clone(),
            environment_id,
            "container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "destination".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(moved.bytes_delta, -2);

    let cleared = blob_store
        .clear(limits, environment_id, "container".to_string())
        .await
        .unwrap();
    assert_eq!(cleared.bytes_delta, -2);
    assert_eq!(cleared.objects_deleted, 1);
}

async fn test_all_deletion_and_replacement_deltas(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();

    for (name, data) in [
        ("zero", &[][..]),
        ("first", &[1, 2, 3, 4]),
        ("second", &[5, 6, 7]),
    ] {
        blob_store
            .write_data(limits.clone(), environment_id, "container", name, data)
            .await
            .unwrap();
    }

    let zero = blob_store
        .delete_object(
            limits.clone(),
            environment_id,
            "container".to_string(),
            "zero".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(zero.bytes_delta, 0);
    assert_eq!(zero.objects_deleted, 1);

    let many = blob_store
        .delete_objects(
            limits.clone(),
            environment_id,
            "container",
            &[
                "first".to_string(),
                "./first".to_string(),
                "missing".to_string(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(many.bytes_delta, -4);
    assert_eq!(many.objects_deleted, 1);
    let already_deleted = blob_store
        .delete_objects(
            limits.clone(),
            environment_id,
            "container",
            &["first".to_string(), "missing".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(already_deleted, BlobStoreMutation::default());

    let cleared = blob_store
        .clear(limits.clone(), environment_id, "container".to_string())
        .await
        .unwrap();
    assert_eq!(cleared.bytes_delta, -3);
    assert_eq!(cleared.objects_deleted, 1);

    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[1, 2, 3, 4, 5],
        )
        .await
        .unwrap();
    let same_path = blob_store
        .move_object(
            limits.clone(),
            environment_id,
            "container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "source".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(same_path, BlobStoreMutation::default());
    assert!(
        blob_store
            .has_object(
                environment_id,
                "container".to_string(),
                "source".to_string(),
            )
            .await
            .unwrap()
    );
    let aliased_same_path = blob_store
        .move_object(
            limits.clone(),
            environment_id,
            "./container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "source".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(aliased_same_path, BlobStoreMutation::default());
    assert!(
        blob_store
            .has_object(
                environment_id,
                "container".to_string(),
                "source".to_string(),
            )
            .await
            .unwrap()
    );
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "destination",
            &[8, 9],
        )
        .await
        .unwrap();
    let copy = blob_store
        .copy_object(
            limits.clone(),
            environment_id,
            "container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "destination".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(copy.bytes_delta, 3);

    let moved = blob_store
        .move_object(
            limits.clone(),
            environment_id,
            "container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "destination".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(moved.bytes_delta, -5);

    let deleted = blob_store
        .delete_container(limits, environment_id, "container".to_string())
        .await
        .unwrap();
    assert_eq!(deleted.bytes_delta, -5);
    assert_eq!(deleted.objects_deleted, 1);
}

#[test]
async fn all_deletion_and_replacement_deltas_in_memory() {
    test_all_deletion_and_replacement_deltas(&in_memory_blob_store()).await;
}

#[test]
async fn all_deletion_and_replacement_deltas_local() {
    let tempdir = TempDir::new().unwrap();
    test_all_deletion_and_replacement_deltas(&fs_blob_store(tempdir.path()).await).await;
}

/// A container name that names the root of the namespace names no container, so `clear` and
/// `delete_container` of such a name give a permanent error, delete nothing and release no
/// quota (`DefaultBlobStoreService::container_path`).
async fn test_root_container_mutations_do_not_release_quota(blob_store: &impl BlobStoreService) {
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(3);
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "object",
            &[1, 2, 3],
        )
        .await
        .unwrap();

    assert!(matches!(
        blob_store
            .clear(limits.clone(), environment_id, String::new())
            .await,
        Err(BlobStoreError::InvalidInput(_))
    ));
    assert!(matches!(
        blob_store
            .delete_container(limits.clone(), environment_id, ".".to_string())
            .await,
        Err(BlobStoreError::InvalidInput(_))
    ));
    assert!(
        blob_store
            .has_object(
                environment_id,
                "container".to_string(),
                "object".to_string(),
            )
            .await
            .unwrap()
    );
    assert!(matches!(
        blob_store
            .write_data(limits, environment_id, "container", "another", &[4],)
            .await,
        Err(BlobStoreError::LimitExceeded(_))
    ));
}

#[test]
async fn root_container_mutations_do_not_release_quota_in_memory() {
    test_root_container_mutations_do_not_release_quota(&in_memory_blob_store()).await;
}

#[test]
async fn root_container_mutations_do_not_release_quota_local() {
    let tempdir = TempDir::new().unwrap();
    test_root_container_mutations_do_not_release_quota(&fs_blob_store(tempdir.path()).await).await;
}

#[test]
async fn move_to_new_destination_reports_no_deleted_object() {
    let blob_store = in_memory_blob_store();
    let environment_id = EnvironmentId::new();
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            unlimited_limits(),
            environment_id,
            "container",
            "source",
            &[1, 2, 3],
        )
        .await
        .unwrap();

    let moved = blob_store
        .move_object(
            unlimited_limits(),
            environment_id,
            "container".to_string(),
            "source".to_string(),
            "container".to_string(),
            "new-destination".to_string(),
        )
        .await
        .unwrap();

    assert_eq!(moved.bytes_delta, 0);
    assert_eq!(moved.objects_deleted, 0);
}

#[test]
async fn concurrent_replacements_keep_accounting_equal_to_physical_size() {
    let blob_store = in_memory_blob_store();
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(10);
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "object",
            &[0; 4],
        )
        .await
        .unwrap();

    let (first, second) = tokio::join!(
        blob_store.write_data(
            limits.clone(),
            environment_id,
            "container",
            "object",
            &[1; 2],
        ),
        blob_store.write_data(
            limits.clone(),
            environment_id,
            "container",
            "object",
            &[2; 5],
        ),
    );
    first.unwrap();
    second.unwrap();

    let physical_size = blob_store
        .object_info(
            environment_id,
            "container".to_string(),
            "object".to_string(),
        )
        .await
        .unwrap()
        .size;
    assert_eq!(limits.blob_storage_delta_for_test(), physical_size as i64);

    let first_lock = blob_store.mutation_lock(environment_id);
    let same_environment_lock = blob_store.mutation_lock(environment_id);
    let other_environment_lock = blob_store.mutation_lock(EnvironmentId::new());
    assert!(Arc::ptr_eq(&first_lock, &same_environment_lock));
    assert!(!Arc::ptr_eq(&first_lock, &other_environment_lock));
}

#[test]
async fn environment_locks_serialize_only_matching_environments() {
    let storage = Arc::new(FailingPutBlobStorage::new());
    let blob_store = Arc::new(DefaultBlobStoreService::new(storage.clone()));
    let first_environment = EnvironmentId::new();
    let other_environment = EnvironmentId::new();
    for environment_id in [first_environment, other_environment] {
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();
    }

    storage.block_next_put();
    let first = tokio::spawn({
        let blob_store = blob_store.clone();
        async move {
            blob_store
                .write_data(
                    unlimited_limits(),
                    first_environment,
                    "container",
                    "first",
                    &[1],
                )
                .await
        }
    });
    storage.wait_for_blocked_put().await;

    let mut same_environment = tokio::spawn({
        let blob_store = blob_store.clone();
        async move {
            blob_store
                .write_data(
                    unlimited_limits(),
                    first_environment,
                    "container",
                    "second",
                    &[2],
                )
                .await
        }
    });
    let other_environment_result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        tokio::spawn({
            let blob_store = blob_store.clone();
            async move {
                blob_store
                    .write_data(
                        unlimited_limits(),
                        other_environment,
                        "container",
                        "other",
                        &[3],
                    )
                    .await
            }
        }),
    )
    .await
    .expect("another environment must not wait for the blocked mutation")
    .unwrap();
    other_environment_result.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut same_environment)
            .await
            .is_err(),
        "the same environment must wait for the blocked mutation"
    );

    storage.release_blocked_put();
    first.await.unwrap().unwrap();
    same_environment.await.unwrap().unwrap();
}

fn registered_blob_entity(
    environment_id: EnvironmentId,
) -> (Arc<EntitySlot>, EntitySlotRegistration) {
    let owner = OwnedAgentId::new(
        environment_id,
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "blob-owner".into(),
        },
    );
    let activation = Arc::new(
        EntityActivation::new(
            ExecutableTarget::new(ComponentId::new(), 1_u64.try_into().unwrap()),
            1_u64.try_into().unwrap(),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name: "blobbody".try_into().unwrap(),
                provision: Default::default(),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
            },
            FilesystemCapability::Incapable,
        )
        .unwrap(),
    );
    let principal = Principal::Agent(golem_common::model::agent::AgentPrincipal {
        agent_id: owner.agent_id.clone(),
    });
    let entity = OwnedAgentEntityId {
        owner,
        entity: activation.entity(),
    };
    let slot = Arc::new(EntitySlot::new(entity.clone()));
    let scope = EntityInvocationScope::new(
        EntityInvocationId::new(entity, OplogIndex::from_u64(2)).unwrap(),
        OplogIndex::from_u64(1),
        activation,
        principal,
        InvocationExecutionMode::Live,
        IdempotencyKey::fresh(),
        false,
        IdempotencyKey::fresh(),
    )
    .unwrap();
    let registration = slot.register(&scope, Default::default()).unwrap();
    (slot, registration)
}

async fn caller_loss_retains_mutation(copy: bool, fail: bool, entity: bool) {
    let storage = Arc::new(FailingPutBlobStorage::new());
    let blob_store = DefaultBlobStoreService::new(storage.clone());
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(10);
    blob_store
        .create_container(environment_id, "container".into())
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[1, 3, 5, 7],
        )
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "destination",
            &[9],
        )
        .await
        .unwrap();
    assert_eq!(limits.blob_storage_delta_for_test(), 5);
    if copy {
        storage.block_next_copy.store(true, Ordering::Release);
    } else {
        storage.block_next_put();
    }
    let (slot, registration) = registered_blob_entity(environment_id);
    let work = async {
        let result = if copy {
            blob_store
                .copy_object(
                    limits.clone(),
                    environment_id,
                    "container".into(),
                    "source".into(),
                    "container".into(),
                    "destination".into(),
                )
                .await
        } else {
            blob_store
                .write_data(
                    limits.clone(),
                    environment_id,
                    "container",
                    "destination",
                    &[1, 3, 5, 7],
                )
                .await
        };
        (
            result.map_err(|error| WorkerExecutorError::runtime(error.to_string())),
            None::<Box<dyn RetainedEntityStore>>,
        )
    };
    let abort = registration.cancellation();
    let mut caller = Box::pin(async {
        if entity {
            await_component_entity_body(abort, work).await
        } else {
            work.await
        }
    });
    assert!(futures::poll!(&mut caller).is_pending());
    storage.wait_for_blocked_put().await;
    assert_eq!(limits.blob_storage_delta_for_test(), 8);
    let mut competing = Box::pin(blob_store.write_data(
        limits.clone(),
        environment_id,
        "container",
        "competing",
        &[2, 4, 6],
    ));
    assert!(futures::poll!(&mut competing).is_pending());
    if entity {
        assert_eq!(slot.fence().len(), 1);
        let (result, retained) = caller.as_mut().await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "Runtime error: Entity body was cancelled"
        );
        assert!(retained.is_none());
    }
    drop(caller);
    drop(registration);
    assert_eq!(slot.active_invocation_count(), 0);
    assert_eq!(
        storage.mutations_dropped.load(Ordering::SeqCst),
        0,
        "caller loss dropped an entered backend mutation after quota reservation"
    );
    assert_eq!(storage.mutations_returned.load(Ordering::SeqCst), 0);
    assert_eq!(limits.blob_storage_delta_for_test(), 8);
    assert!(
        futures::poll!(&mut competing).is_pending(),
        "caller loss released the unsettled environment lock"
    );
    drop(competing);

    if copy {
        storage.fail_next_copy.store(fail, Ordering::Release);
    } else {
        storage.fail_next_put.store(fail, Ordering::Release);
    }
    storage.release_blocked_put();
    // The original lock can become available only after the service settled the result.
    let settled = blob_store.mutation_lock(environment_id).lock_owned().await;
    assert_eq!(storage.mutations_returned.load(Ordering::SeqCst), 1);
    assert_eq!(
        storage.mutations_failed.load(Ordering::SeqCst),
        usize::from(fail)
    );
    assert_eq!(storage.mutations_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(
        limits.blob_storage_delta_for_test(),
        if fail { 5 } else { 8 }
    );
    let physical = blob_store
        .get_data(
            environment_id,
            "container".into(),
            "destination".into(),
            0,
            if fail { 0 } else { 3 },
        )
        .await
        .unwrap();
    assert_eq!(physical, if fail { vec![9] } else { vec![1, 3, 5, 7] });
    drop(settled);
    let contender = blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "competing",
            &[2, 4, 6],
        )
        .await;
    if fail {
        assert_eq!(contender.unwrap().bytes_delta, 3);
    } else {
        assert!(matches!(contender, Err(BlobStoreError::LimitExceeded(_))));
    }
    assert_eq!(
        limits.blob_storage_delta_for_test(),
        8,
        "known failure must refund exactly once"
    );
    assert_eq!(storage.mutations_returned.load(Ordering::SeqCst), 1);
}

macro_rules! caller_loss_case {
    ($name:ident, $copy:expr, $fail:expr, $entity:expr) => {
        #[test]
        #[test_r::timeout("30s")]
        async fn $name() {
            caller_loss_retains_mutation($copy, $fail, $entity).await;
        }
    };
}
caller_loss_case!(
    dropped_write_caller_retains_success_settlement,
    false,
    false,
    false
);
caller_loss_case!(
    dropped_write_caller_retains_failure_settlement,
    false,
    true,
    false
);
caller_loss_case!(
    dropped_copy_caller_retains_success_settlement,
    true,
    false,
    false
);
caller_loss_case!(
    dropped_copy_caller_retains_failure_settlement,
    true,
    true,
    false
);
caller_loss_case!(
    fenced_entity_write_retains_success_settlement,
    false,
    false,
    true
);
caller_loss_case!(
    fenced_entity_write_retains_failure_settlement,
    false,
    true,
    true
);
caller_loss_case!(
    fenced_entity_copy_retains_success_settlement,
    true,
    false,
    true
);
caller_loss_case!(
    fenced_entity_copy_retains_failure_settlement,
    true,
    true,
    true
);

async fn unknown_mutation_fixture() -> (
    Arc<FailingPutBlobStorage>,
    Arc<DefaultBlobStoreService>,
    EnvironmentId,
    Arc<AtomicResourceEntry>,
) {
    let storage = Arc::new(FailingPutBlobStorage::new());
    let blob_store = Arc::new(DefaultBlobStoreService::new(storage.clone()));
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(10);
    blob_store
        .create_container(environment_id, "container".into())
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[1, 3, 5, 7],
        )
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "destination",
            &[9],
        )
        .await
        .unwrap();
    assert_eq!(limits.blob_storage_delta_for_test(), 5);
    (storage, blob_store, environment_id, limits)
}

fn assert_owned_failure(
    blob_store: &DefaultBlobStoreService,
    environment_id: EnvironmentId,
    original: &Arc<tokio::sync::Mutex<()>>,
    operation: &'static str,
    cause: BlobMutationFailureCause,
) -> BlobMutationFailure {
    let failure = blob_store
        .mutation_failure(environment_id)
        .expect("unknown result must retain typed service health");
    assert_eq!(failure.environment_id, environment_id);
    assert_eq!(failure.operation, operation);
    assert_eq!(failure.cause, cause);
    let owner = blob_store.mutation_owner(environment_id);
    let retained = owner.failure.lock().unwrap();
    let retained = retained.as_ref().unwrap();
    assert_eq!(retained.failure, failure);
    assert!(
        Arc::ptr_eq(
            tokio::sync::OwnedMutexGuard::mutex(&retained._guard),
            original
        ),
        "retained health must own the original guard"
    );
    assert!(
        original.clone().try_lock_owned().is_err(),
        "health inspection must not unlock unknown work"
    );
    failure
}

fn unavailable(result: Result<BlobStoreMutation, BlobStoreError>) -> BlobMutationFailure {
    match result {
        Err(BlobStoreError::Unavailable(failure)) => failure,
        other => panic!("expected typed unavailable admission, got {other:?}"),
    }
}

async fn lost_mutation_task_retains_health(copy: bool) {
    let (storage, blob_store, environment_id, limits) = unknown_mutation_fixture().await;
    let original = blob_store.mutation_owner(environment_id).lock.clone();
    let storage_thread = storage.clone();
    let service_thread = blob_store.clone();
    let limits_thread = limits.clone();
    let mut queued = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (queued,) = runtime.block_on(async move {
            if copy {
                storage_thread
                    .block_next_copy
                    .store(true, Ordering::Release);
            } else {
                storage_thread.block_next_put();
            }
            let mut caller = Box::pin(async {
                if copy {
                    service_thread
                        .copy_object(
                            limits_thread.clone(),
                            environment_id,
                            "container".into(),
                            "source".into(),
                            "container".into(),
                            "destination".into(),
                        )
                        .await
                } else {
                    service_thread
                        .write_data(
                            limits_thread.clone(),
                            environment_id,
                            "container",
                            "destination",
                            &[1, 3, 5, 7],
                        )
                        .await
                }
            });
            assert!(futures::poll!(&mut caller).is_pending());
            storage_thread.wait_for_blocked_put().await;
            assert_eq!(limits_thread.blob_storage_delta_for_test(), 8);
            assert!(service_thread.mutation_failure(environment_id).is_none());
            let service_queued = service_thread.clone();
            let limits_queued = limits_thread.clone();
            let mut queued = Box::pin(async move {
                service_queued
                    .clear(limits_queued, environment_id, "container".into())
                    .await
            });
            assert!(futures::poll!(&mut queued).is_pending());
            drop(caller);
            assert_eq!(storage_thread.mutations_dropped.load(Ordering::SeqCst), 0);
            (queued,)
        });
        // Destroy the actual operation runtime after its backend is Pending. The caller
        // has already detached; no task handle or production test hook is exposed.
        drop(runtime);
        queued
    })
    .join()
    .unwrap();
    let failure = assert_owned_failure(
        &blob_store,
        environment_id,
        &original,
        if copy { "copy_object" } else { "write_data" },
        BlobMutationFailureCause::TaskLost,
    );
    match futures::poll!(&mut queued) {
        std::task::Poll::Ready(result) => assert_eq!(unavailable(result), failure),
        std::task::Poll::Pending => {
            panic!("lost task did not notify the already-queued mutator")
        }
    }
    let mut later =
        Box::pin(blob_store.delete_container(limits.clone(), environment_id, "container".into()));
    match futures::poll!(&mut later) {
        std::task::Poll::Ready(result) => assert_eq!(unavailable(result), failure),
        std::task::Poll::Pending => panic!("later mutator waited on a lost operation"),
    }
    assert_eq!(limits.blob_storage_delta_for_test(), 8);
    assert_eq!(storage.mutations_returned.load(Ordering::SeqCst), 0);
    assert_eq!(storage.mutations_dropped.load(Ordering::SeqCst), 1);
}

#[test]
#[test_r::timeout("30s")]
async fn lost_write_task_retains_owned_health_and_notifies_admission() {
    lost_mutation_task_retains_health(false).await;
}

#[test]
#[test_r::timeout("30s")]
async fn lost_copy_task_retains_owned_health_and_notifies_admission() {
    lost_mutation_task_retains_health(true).await;
}

async fn entered_mutation_settles_after_stop(copy: bool, fail: bool) {
    let storage = Arc::new(FailingPutBlobStorage::new());
    let blob_store = DefaultBlobStoreService::new(storage.clone());
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(10);
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "source",
            &[1, 3, 5, 7],
        )
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "destination",
            &[9],
        )
        .await
        .unwrap();
    assert_eq!(limits.blob_storage_delta_for_test(), 5);

    if copy {
        storage.block_next_copy.store(true, Ordering::Release);
        storage.fail_next_copy.store(fail, Ordering::Release);
    } else {
        storage.block_next_put();
        storage.fail_next_put.store(fail, Ordering::Release);
    }
    let provider = async {
        if copy {
            blob_store
                .copy_object(
                    limits.clone(),
                    environment_id,
                    "container".to_string(),
                    "source".to_string(),
                    "container".to_string(),
                    "destination".to_string(),
                )
                .await
        } else {
            blob_store
                .write_data(
                    limits.clone(),
                    environment_id,
                    "container",
                    "destination",
                    &[1, 3, 5, 7],
                )
                .await
        }
    };
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mut mutation = std::pin::pin!(await_blob_provider(
        BlobProviderOperation::Mutation,
        async { stopped.await.unwrap() },
        provider,
    ));
    assert!(futures::poll!(&mut mutation).is_pending());
    // put/copy is now Pending, after the production service reserved the growth.
    storage.wait_for_blocked_put().await;
    assert_eq!(limits.blob_storage_delta_for_test(), 8);
    let mut competing = std::pin::pin!(blob_store.write_data(
        limits.clone(),
        environment_id,
        "container",
        "competing",
        &[2, 4, 6],
    ));
    assert!(futures::poll!(&mut competing).is_pending());

    stop.send(InterruptKind::Suspend(
        golem_common::model::Timestamp::now_utc(),
    ))
    .unwrap();
    assert!(
        futures::poll!(&mut mutation).is_pending(),
        "interruption dropped an entered {} with three reserved bytes while the backend was still Pending",
        if copy { "copy" } else { "write" },
    );
    assert!(
        futures::poll!(&mut competing).is_pending(),
        "the environment lock must survive stop"
    );
    assert_eq!(limits.blob_storage_delta_for_test(), 8);
    assert_eq!(
        blob_store
            .get_data(
                environment_id,
                "container".to_string(),
                "destination".to_string(),
                0,
                0
            )
            .await
            .unwrap(),
        [9]
    );

    storage.release_blocked_put();
    let settled = mutation.await.unwrap();
    if fail {
        assert!(matches!(settled, Err(BlobStoreError::TransientBackend(_))));
        assert_eq!(limits.blob_storage_delta_for_test(), 5);
        assert_eq!(competing.await.unwrap().bytes_delta, 3);
        assert_eq!(
            blob_store
                .get_data(
                    environment_id,
                    "container".to_string(),
                    "destination".to_string(),
                    0,
                    0
                )
                .await
                .unwrap(),
            [9]
        );
    } else {
        assert_eq!(settled.unwrap().bytes_delta, 3);
        assert_eq!(limits.blob_storage_delta_for_test(), 8);
        assert!(matches!(
            competing.await,
            Err(BlobStoreError::LimitExceeded(_))
        ));
        assert_eq!(
            blob_store
                .get_data(
                    environment_id,
                    "container".to_string(),
                    "destination".to_string(),
                    0,
                    3
                )
                .await
                .unwrap(),
            [1, 3, 5, 7]
        );
    }
    assert_eq!(limits.blob_storage_delta_for_test(), 8);
}

#[test]
#[test_r::timeout("30s")]
async fn entered_write_settles_after_stop() {
    entered_mutation_settles_after_stop(false, false).await;
}

#[test]
#[test_r::timeout("30s")]
async fn entered_write_failure_refunds_after_stop() {
    entered_mutation_settles_after_stop(false, true).await;
}

#[test]
#[test_r::timeout("30s")]
async fn entered_copy_settles_after_stop() {
    entered_mutation_settles_after_stop(true, false).await;
}

#[test]
#[test_r::timeout("30s")]
async fn entered_copy_failure_refunds_after_stop() {
    entered_mutation_settles_after_stop(true, true).await;
}

#[test]
async fn failed_writes_restore_reservations_without_publishing_credit() {
    let storage = Arc::new(FailingPutBlobStorage::new());
    let blob_store = DefaultBlobStoreService::new(storage.clone());
    let environment_id = EnvironmentId::new();
    let limits = unlimited_limits();
    limits.set_available_blob_storage_bytes(10);
    blob_store
        .create_container(environment_id, "container".to_string())
        .await
        .unwrap();
    blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "object",
            &[0; 5],
        )
        .await
        .unwrap();

    storage.fail_next_put();
    let failed_growth = blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "another",
            &[1; 3],
        )
        .await;
    assert!(matches!(
        failed_growth,
        Err(BlobStoreError::TransientBackend(_))
    ));
    assert_eq!(limits.blob_storage_delta_for_test(), 5);

    storage.fail_next_put();
    let failed = blob_store
        .write_data(
            limits.clone(),
            environment_id,
            "container",
            "object",
            &[1; 2],
        )
        .await;
    assert!(matches!(failed, Err(BlobStoreError::TransientBackend(_))));
    assert_eq!(limits.blob_storage_delta_for_test(), 5);

    let rejected = blob_store
        .write_data(limits, environment_id, "container", "another", &[2; 6])
        .await;
    assert!(matches!(rejected, Err(BlobStoreError::LimitExceeded(_))));
}
