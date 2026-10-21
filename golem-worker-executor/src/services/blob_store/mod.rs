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

use crate::services::resource_limits::AtomicResourceEntry;
use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::types::ObjectMetadata;
use golem_service_base::storage::blob::{
    BlobFailure, BlobMissingError, BlobNameError, BlobStorage, BlobStorageLabelledApi,
    BlobStorageNamespace, ExistsResult, blob_file_name_to_string, blob_path_is_root,
    join_blob_path, normalized_blob_path_text,
};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard};
use tracing::Instrument;

fn signed_delta(new_size: u64, old_size: u64) -> i64 {
    if new_size >= old_size {
        new_size.saturating_sub(old_size).min(i64::MAX as u64) as i64
    } else {
        -(old_size.saturating_sub(new_size).min(i64::MAX as u64) as i64)
    }
}

/// Typed errors for blob store operations, enabling semantic retry classification
#[derive(Debug, Clone)]
pub enum BlobStoreError {
    /// The requested object or container was not found
    NotFound(String),
    /// The container or object already exists
    AlreadyExists(String),
    /// Permission denied
    PermissionDenied(String),
    /// Invalid input (bad name, bad range, etc.)
    InvalidInput(String),
    /// The account's blob storage byte limit would be exceeded
    LimitExceeded(String),
    /// Transient backend failure (network, timeout, etc.)
    TransientBackend(String),
    /// Other/unknown error
    Other(String),
    /// Storage admission is unavailable because an earlier mutation has no known result.
    Unavailable(BlobMutationFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobMutationFailureCause {
    TaskLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMutationFailure {
    pub environment_id: EnvironmentId,
    pub operation: &'static str,
    pub cause: BlobMutationFailureCause,
}

impl std::fmt::Display for BlobMutationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Blob mutation unavailable: environment {}, operation {}, unknown outcome after {:?}",
            self.environment_id, self.operation, self.cause
        )
    }
}

impl std::error::Error for BlobMutationFailure {}

pub(crate) fn require_known_blob_result<T>(
    result: Result<T, BlobStoreError>,
) -> Result<Result<T, BlobStoreError>, BlobMutationFailure> {
    match result {
        Err(BlobStoreError::Unavailable(failure)) => Err(failure),
        result => Ok(result),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlobStoreMutation {
    pub bytes_delta: i64,
    pub objects_deleted: u64,
}

impl std::fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::AlreadyExists(msg) => write!(f, "Already exists: {msg}"),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
            Self::LimitExceeded(msg) => write!(f, "Limit exceeded: {msg}"),
            Self::TransientBackend(msg) => write!(f, "Backend error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
            Self::Unavailable(failure) => std::fmt::Display::fmt(failure, f),
        }
    }
}

/// Interface for storing blobs in a persistent storage.
///
/// A guest picks the container name, and a name that names the root of the namespace names no
/// container. Every method that takes a container name gives [`BlobStoreError::InvalidInput`]
/// for such a name, which is permanent (`DefaultBlobStoreService::container_path`).
#[async_trait]
pub trait BlobStoreService: Send + Sync {
    async fn clear(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError>;

    /// Writes the source object to `destination_container_name` and
    /// `destination_object_name`, and keeps the source object.
    ///
    /// A source object that is not there gives [`BlobStoreError::NotFound`], which is permanent,
    /// so the guest gets it at once, the executor does not retry it, and the operation writes
    /// nothing.
    async fn copy_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_container(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_objects(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError>;

    /// Reads the bytes from `start` to `end` of an object. Both offsets are inclusive, so the
    /// result has `end - start + 1` bytes.
    ///
    /// A range with a byte that is not in the object gives [`BlobStoreError::InvalidInput`],
    /// which is permanent, so the caller gets it on the first attempt: an `end` at or after the
    /// size of the object, a `start` after `end`, and each range of an empty object.
    async fn get_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobStoreError>;

    async fn has_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<bool, BlobStoreError>;

    async fn list_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Vec<String>, BlobStoreError>;

    /// Writes the source object to `destination_container_name` and
    /// `destination_object_name`, and then deletes the source object.
    ///
    /// The write comes before the delete, so a source object that is not there gives the same
    /// permanent [`BlobStoreError::NotFound`] as [`BlobStoreService::copy_object`], and the
    /// operation deletes nothing.
    async fn move_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError>;

    async fn write_data(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<BlobStoreMutation, BlobStoreError>;
}

pub struct DefaultBlobStoreService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    mutation_owners: StdMutex<HashMap<EnvironmentId, Arc<EnvironmentMutationOwner>>>,
}

struct RetainedMutationFailure {
    failure: BlobMutationFailure,
    _guard: OwnedMutexGuard<()>,
}

struct EnvironmentMutationOwner {
    environment_id: EnvironmentId,
    lock: Arc<Mutex<()>>,
    failure: StdMutex<Option<RetainedMutationFailure>>,
    failed: Notify,
}

impl EnvironmentMutationOwner {
    fn failure(&self) -> Option<BlobMutationFailure> {
        self.failure
            .lock()
            .unwrap()
            .as_ref()
            .map(|retained| retained.failure.clone())
    }

    fn retain_unknown(&self, failure: BlobMutationFailure, guard: OwnedMutexGuard<()>) {
        *self.failure.lock().unwrap() = Some(RetainedMutationFailure {
            failure,
            _guard: guard,
        });
        self.failed.notify_waiters();
    }

    async fn enter(
        self: &Arc<Self>,
        operation: &'static str,
    ) -> Result<BlobMutationOwnership, BlobStoreError> {
        let mut failed = std::pin::pin!(self.failed.notified());
        failed.as_mut().enable();
        if let Some(failure) = self.failure() {
            return Err(BlobStoreError::Unavailable(failure));
        }
        let guard = tokio::select! {
            biased;
            _ = &mut failed => {
                return Err(BlobStoreError::Unavailable(self.failure().expect("published mutation failure missing")));
            }
            guard = self.lock.clone().lock_owned() => guard,
        };
        if let Some(failure) = self.failure() {
            return Err(BlobStoreError::Unavailable(failure));
        }
        Ok(BlobMutationOwnership {
            environment: self.clone(),
            operation,
            guard: Some(guard),
        })
    }
}

struct BlobMutationOwnership {
    environment: Arc<EnvironmentMutationOwner>,
    operation: &'static str,
    guard: Option<OwnedMutexGuard<()>>,
}

impl BlobMutationOwnership {
    fn complete(mut self) {
        drop(self.guard.take());
    }

    fn retain_failure(&mut self, cause: BlobMutationFailureCause) {
        if let Some(guard) = self.guard.take() {
            self.environment.retain_unknown(
                BlobMutationFailure {
                    environment_id: self.environment.environment_id,
                    operation: self.operation,
                    cause,
                },
                guard,
            );
        }
    }
}

impl Drop for BlobMutationOwnership {
    fn drop(&mut self) {
        self.retain_failure(BlobMutationFailureCause::TaskLost);
    }
}

async fn await_owned_blob_mutation(
    ownership: BlobMutationOwnership,
    work: impl Future<Output = Result<BlobStoreMutation, BlobStoreError>> + Send + 'static,
) -> Result<BlobStoreMutation, BlobStoreError> {
    let task = tokio::spawn(
        async move {
            let result = work.await;
            ownership.complete();
            result
        }
        .in_current_span(),
    );
    // JoinHandle Drop detaches the operation. Entity-body cancellation may destroy the
    // caller, but storage, its original lock and quota settlement still have this owner.
    match task.await {
        Ok(result) => result,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => panic!("Blob mutation task ended without a backend result: {error}"),
    }
}

/// Gives the `BlobStoreError` of an error of the blob storage.
///
/// [`BlobFailure::of`] tells a permanent error from a transient one. A transient error becomes
/// [`BlobStoreError::TransientBackend`], so the executor retries the operation. A refusal of the
/// storage (a [`BlobRefusedError`](golem_service_base::storage::blob::BlobRefusedError)) is not
/// an error of the input of the guest, and a retry gets the same answer until an operator fixes
/// the storage, so it becomes [`BlobStoreError::PermissionDenied`], which is permanent. Its
/// message names the refusal and its cause. Of the permanent
/// errors, a [`BlobMissingError`] becomes [`BlobStoreError::NotFound`], and a
/// [`BlobRangeError`](golem_service_base::storage::blob::BlobRangeError) and a
/// [`BlobNameError`] are errors of the input of the guest, so they become
/// [`BlobStoreError::InvalidInput`]. `classify_blob_store_error` in
/// `crate::durable_host::blobstore` makes each of the two permanent. Each method of
/// [`DefaultBlobStoreService`] maps its errors with this function, so an error of the input is
/// permanent at each of them. The message of a permanent error is the message of the typed
/// error, which is the root cause, without a context around it.
///
/// [`BlobFailure::of`] has one downcast for [`BlobNameError`] and one rule: each variant of that
/// error is a name that the guest chose and that the storage cannot use, so each of them is
/// permanent, whichever backend gives it. The path rules are in it too, so a `..` name and an absolute name are
/// permanent like a name that S3 does not accept as an object key.
///
/// [`BlobMissingError`] is not a name error: the storage accepts the name, and holds no blob at it.
/// `copy` of the blob storage gives it for a source path with no blob at it on the in-memory, the
/// SQLite and the S3 backends, and on each backend for a copy onto the same path. The filesystem
/// backend gives the error of the filesystem for a copy to another path. `move` is a copy and then
/// a delete, so [`BlobStoreService::copy_object`] and [`BlobStoreService::move_object`] give
/// [`BlobStoreError::NotFound`] for a source object that the guest names and that is not there,
/// where the storage gives [`BlobMissingError`]. A retry cannot make the storage hold that object,
/// so the error is permanent.
fn blob_store_error(err: anyhow::Error) -> BlobStoreError {
    match BlobFailure::of(&err) {
        BlobFailure::Transient => BlobStoreError::TransientBackend(err.to_string()),
        BlobFailure::Refused(_) => BlobStoreError::PermissionDenied(format!("{err:#}")),
        BlobFailure::Permanent if err.is::<BlobMissingError>() => {
            BlobStoreError::NotFound(err.root_cause().to_string())
        }
        BlobFailure::Permanent => BlobStoreError::InvalidInput(err.root_cause().to_string()),
    }
}

/// Gives the [`BlobStoreError::InvalidInput`] of a [`BlobNameError`], which is permanent.
fn name_error(err: BlobNameError) -> BlobStoreError {
    BlobStoreError::InvalidInput(err.to_string())
}

impl DefaultBlobStoreService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self {
            blob_storage,
            mutation_owners: StdMutex::new(HashMap::new()),
        }
    }

    fn mutation_owner(&self, environment_id: EnvironmentId) -> Arc<EnvironmentMutationOwner> {
        let mut owners = self.mutation_owners.lock().unwrap();
        owners.retain(|_, owner| {
            Arc::strong_count(owner) > 1
                || owner.failure().is_some()
                || Arc::strong_count(&owner.lock) > 1
        });
        owners
            .entry(environment_id)
            .or_insert_with(|| {
                Arc::new(EnvironmentMutationOwner {
                    environment_id,
                    lock: Arc::new(Mutex::new(())),
                    failure: StdMutex::new(None),
                    failed: Notify::new(),
                })
            })
            .clone()
    }

    pub fn mutation_failure(&self, environment_id: EnvironmentId) -> Option<BlobMutationFailure> {
        self.mutation_owners
            .lock()
            .unwrap()
            .get(&environment_id)
            .and_then(|owner| owner.failure())
    }

    #[cfg(test)]
    fn mutation_lock(&self, environment_id: EnvironmentId) -> Arc<Mutex<()>> {
        self.mutation_owner(environment_id).lock.clone()
    }

    /// Gives the path of the container, or [`BlobStoreError::InvalidInput`] for a container name
    /// that names the root of the namespace, that is absolute, or that has a `..` name.
    ///
    /// A guest picks the container name, and a name with no name in it, for example an empty
    /// name or `.`, names the root (`blob_path_is_root`). The root is the namespace itself and
    /// not a container, so every method that takes a container name reads it here first. Without
    /// the rule each method answers about the namespace: `container_exists` tells the guest that
    /// the root is a container, `delete_container` reports that it removed one, and
    /// `copy_object` and `move_object` read and write a blob beside the containers instead of
    /// inside one.
    ///
    /// An absolute name and a name with a `..` name leave the namespace (`join_blob_path`). The
    /// rule is here too, so an operation that sends no path to the storage, for example
    /// `delete_objects` of no object, refuses such a name as well.
    ///
    /// The error is permanent (`classify_blob_store_error`), so the guest gets it on the first
    /// call and the executor does not retry a name that can never name a container. The error
    /// names the container as the guest wrote it, because the guest reads the message.
    ///
    /// A container name that breaks another rule of a name is not here. The path of the
    /// operation holds the container name and the object name, and the backend that reads that
    /// path gives the rule that it breaks, with both names in the message.
    fn container_path(container_name: &str) -> Result<&Path, BlobStoreError> {
        let path = Path::new(container_name);

        if blob_path_is_root(path) {
            return Err(name_error(BlobNameError::NoName {
                path: path.to_path_buf(),
            }));
        }
        join_blob_path(container_name, "").map_err(name_error)?;

        Ok(path)
    }

    /// Gives the path of the object in the container, with `/` between the two names on every
    /// host, or [`BlobStoreError::InvalidInput`].
    ///
    /// The container name follows the rules of `container_path`. An object name that is absolute
    /// or that has a `..` name gives the error too, so an object name never replaces or leaves
    /// its container (`join_blob_path`). A root object name cannot name an object.
    fn object_path(container_name: &str, object_name: &str) -> Result<PathBuf, BlobStoreError> {
        Self::container_path(container_name)?;
        if blob_path_is_root(Path::new(object_name)) {
            return Err(name_error(BlobNameError::NoName {
                path: PathBuf::from(object_name),
            }));
        }
        join_blob_path(container_name, object_name).map_err(name_error)
    }

    /// Gives the text of the one form of the path, so two paths that name one blob give the
    /// same text.
    fn blob_path_identity(path: &Path) -> Result<String, BlobStoreError> {
        normalized_blob_path_text(path).map_err(name_error)
    }
}

#[async_trait]
impl BlobStoreService for DefaultBlobStoreService {
    async fn clear(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self.mutation_owner(environment_id).enter("clear").await?;
        let storage = self.blob_storage.clone();
        await_owned_blob_mutation(mutation, async move {
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let path = Self::container_path(&container_name)?;
            let blob_storage = storage.with("blob_store", "clear");
            if blob_storage
                .exists(namespace.clone(), path)
                .await
                .map_err(blob_store_error)?
                == ExistsResult::DoesNotExist
            {
                blob_storage
                    .create_dir(namespace, path)
                    .await
                    .map_err(blob_store_error)?;
                return Ok(BlobStoreMutation::default());
            }
            let blobs = blob_storage
                .list_blobs_below(namespace.clone(), path)
                .await
                .map_err(blob_store_error)?;
            let deleted_bytes = blobs
                .iter()
                .fold(0u64, |sum, blob| sum.saturating_add(blob.size));
            if blob_storage
                .delete_dir(namespace.clone(), path)
                .await
                .map_err(blob_store_error)?
                .is_none()
            {
                return Ok(BlobStoreMutation::default());
            }
            // Re-create the empty container directory so the container continues to exist.
            // clear() semantics: remove all objects, keep the container itself.
            blob_storage
                .create_dir(namespace, path)
                .await
                .map_err(blob_store_error)?;
            let mutation = BlobStoreMutation {
                bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
                objects_deleted: blobs.len() as u64,
            };
            let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
            debug_assert!(recorded);
            Ok(mutation)
        })
        .await
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "container_exists")
            .exists(BlobStorageNamespace::CustomStorage { environment_id }, path)
            .await
            .map_err(blob_store_error)
            .map(|result| match result {
                ExistsResult::Directory => true,
                ExistsResult::File => false,
                ExistsResult::DoesNotExist => false,
            })
    }

    async fn copy_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("copy_object")
            .await?;
        let storage = self.blob_storage.clone();
        await_owned_blob_mutation(mutation, async move {
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let source = Self::object_path(&source_container_name, &source_object_name)?;
            let destination =
                Self::object_path(&destination_container_name, &destination_object_name)?;
            let blob_storage = storage.with("blob_store", "copy_object");
            let source_size = blob_storage
                .get_metadata(namespace.clone(), &source)
                .await
                .map_err(blob_store_error)?
                .ok_or_else(|| {
                    BlobStoreError::NotFound(
                        BlobMissingError {
                            path: source.clone(),
                        }
                        .to_string(),
                    )
                })?
                .size;
            let old_destination_size = blob_storage
                .get_metadata(namespace.clone(), &destination)
                .await
                .map_err(blob_store_error)?
                .map_or(0, |metadata| metadata.size);
            let delta = signed_delta(source_size, old_destination_size);
            if delta > 0 && !resource_limits.try_record_blob_storage_delta(delta) {
                return Err(BlobStoreError::LimitExceeded(
                    "Blob storage byte limit exceeded".to_string(),
                ));
            }
            if let Err(error) = blob_storage.copy(namespace, &source, &destination).await {
                if delta > 0 {
                    resource_limits.rollback_blob_storage_delta(delta);
                }
                return Err(blob_store_error(error));
            }
            if delta < 0 {
                let recorded = resource_limits.try_record_blob_storage_delta(delta);
                debug_assert!(recorded);
            }
            Ok(BlobStoreMutation {
                bytes_delta: delta,
                objects_deleted: 0,
            })
        })
        .await
    }

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("create_container")
            .await?;
        let storage = self.blob_storage.clone();
        await_owned_blob_mutation(mutation, async move {
            let path = Self::container_path(&container_name)?;
            storage
                .with("blob_store", "create_container")
                .create_dir(BlobStorageNamespace::CustomStorage { environment_id }, path)
                .await
                .map_err(blob_store_error)?;
            Ok(BlobStoreMutation::default())
        })
        .await
    }

    async fn delete_container(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("delete_container")
            .await?;
        let storage = self.blob_storage.clone();
        await_owned_blob_mutation(mutation, async move {
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let path = Self::container_path(&container_name)?;
            let blob_storage = storage.with("blob_store", "delete_container");
            if blob_storage
                .exists(namespace.clone(), path)
                .await
                .map_err(blob_store_error)?
                == ExistsResult::DoesNotExist
            {
                return Ok(BlobStoreMutation::default());
            }
            let blobs = blob_storage
                .list_blobs_below(namespace.clone(), path)
                .await
                .map_err(blob_store_error)?;
            let deleted_bytes = blobs
                .iter()
                .fold(0u64, |sum, blob| sum.saturating_add(blob.size));
            if blob_storage
                .delete_dir(namespace, path)
                .await
                .map_err(blob_store_error)?
                .is_none()
            {
                return Ok(BlobStoreMutation::default());
            }
            let mutation = BlobStoreMutation {
                bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
                objects_deleted: blobs.len() as u64,
            };
            let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
            debug_assert!(recorded);
            Ok(mutation)
        })
        .await
    }

    async fn delete_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("delete_object")
            .await?;
        let storage = self.blob_storage.clone();
        await_owned_blob_mutation(mutation, async move {
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let path = Self::object_path(&container_name, &object_name)?;
            let blob_storage = storage.with("blob_store", "delete_object");
            let old_metadata = blob_storage
                .get_metadata(namespace.clone(), &path)
                .await
                .map_err(blob_store_error)?;
            let old_size = old_metadata.as_ref().map_or(0, |metadata| metadata.size);
            blob_storage
                .delete(namespace, &path)
                .await
                .map_err(blob_store_error)?;
            let mutation = BlobStoreMutation {
                bytes_delta: -(old_size.min(i64::MAX as u64) as i64),
                objects_deleted: u64::from(old_metadata.is_some()),
            };
            let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
            debug_assert!(recorded);
            Ok(mutation)
        })
        .await
    }

    async fn delete_objects(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("delete_objects")
            .await?;
        let storage = self.blob_storage.clone();
        let container_name = container_name.to_owned();
        let object_names = object_names.to_vec();
        await_owned_blob_mutation(mutation, async move {
            Self::container_path(&container_name)?;
            let paths: HashMap<String, PathBuf> = object_names
                .iter()
                .map(|object_name| {
                    let path = Self::object_path(&container_name, object_name)?;
                    Ok((Self::blob_path_identity(&path)?, path))
                })
                .collect::<Result<_, _>>()?;
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let mut deleted_bytes = 0u64;
            let mut existing_paths = Vec::new();
            let blob_storage = storage.with("blob_store", "delete_objects");
            for path in paths.values() {
                if let Some(metadata) = blob_storage
                    .get_metadata(namespace.clone(), path)
                    .await
                    .map_err(blob_store_error)?
                {
                    deleted_bytes = deleted_bytes.saturating_add(metadata.size);
                    existing_paths.push(path.clone());
                }
            }
            if existing_paths.is_empty() {
                return Ok(BlobStoreMutation::default());
            }
            blob_storage
                .delete_many(namespace, &existing_paths)
                .await
                .map_err(blob_store_error)?;
            let mutation = BlobStoreMutation {
                bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
                objects_deleted: existing_paths.len() as u64,
            };
            let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
            debug_assert!(recorded);
            Ok(mutation)
        })
        .await
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "get_container")
            .get_metadata(BlobStorageNamespace::CustomStorage { environment_id }, path)
            .await
            .map_err(blob_store_error)
            .map(|result| result.map(|metadata| metadata.last_modified_at.to_millis()))
    }

    async fn get_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        Self::container_path(&container_name)?;
        let path = join_blob_path(&container_name, &object_name).map_err(name_error)?;
        let data = self
            .blob_storage
            .with("blob_store", "get_data")
            .get_raw_slice(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
                start,
                end,
            )
            .await
            .map_err(blob_store_error)?;

        match data {
            Some(data) => Ok(data.to_vec()),
            None => Err(BlobStoreError::NotFound(
                "Object does not exist".to_string(),
            )),
        }
    }

    async fn has_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<bool, BlobStoreError> {
        let path = Self::object_path(&container_name, &object_name)?;
        self.blob_storage
            .with("blob_store", "has_object")
            .exists(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(blob_store_error)
            .map(|result| match result {
                ExistsResult::Directory => false,
                ExistsResult::File => true,
                ExistsResult::DoesNotExist => false,
            })
    }

    async fn list_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Vec<String>, BlobStoreError> {
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "list_objects")
            .list_dir(BlobStorageNamespace::CustomStorage { environment_id }, path)
            .await
            .map_err(blob_store_error)
            .and_then(|paths| {
                paths
                    .iter()
                    .map(|path| blob_file_name_to_string(path).map_err(name_error))
                    .collect()
            })
    }

    async fn move_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("move_object")
            .await?;
        let storage = self.blob_storage.clone();
        await_owned_blob_mutation(mutation, async move {
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let source = Self::object_path(&source_container_name, &source_object_name)?;
            let destination =
                Self::object_path(&destination_container_name, &destination_object_name)?;
            let blob_storage = storage.with("blob_store", "move_object");
            if Self::blob_path_identity(&source)? == Self::blob_path_identity(&destination)? {
                blob_storage
                    .get_metadata(namespace, &source)
                    .await
                    .map_err(blob_store_error)?
                    .ok_or_else(|| {
                        BlobStoreError::NotFound(
                            BlobMissingError {
                                path: source.clone(),
                            }
                            .to_string(),
                        )
                    })?;
                return Ok(BlobStoreMutation::default());
            }
            let old_destination = blob_storage
                .get_metadata(namespace.clone(), &destination)
                .await
                .map_err(blob_store_error)?;
            let old_destination_size = old_destination.as_ref().map_or(0, |metadata| metadata.size);
            blob_storage
                .r#move(namespace, &source, &destination)
                .await
                .map_err(blob_store_error)?;
            let mutation = BlobStoreMutation {
                bytes_delta: -(old_destination_size.min(i64::MAX as u64) as i64),
                objects_deleted: u64::from(old_destination.is_some()),
            };
            let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
            debug_assert!(recorded);
            Ok(mutation)
        })
        .await
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        let path = Self::object_path(&container_name, &object_name)?;
        match self
            .blob_storage
            .with("blob_store", "object_info")
            .get_metadata(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(blob_store_error)?
        {
            Some(metadata) => Ok(ObjectMetadata {
                name: object_name,
                container: container_name,
                created_at: metadata.last_modified_at.to_millis(),
                size: metadata.size,
            }),
            None => Err(BlobStoreError::NotFound(
                "Object does not exist".to_string(),
            )),
        }
    }

    async fn write_data(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let mutation = self
            .mutation_owner(environment_id)
            .enter("write_data")
            .await?;
        let storage = self.blob_storage.clone();
        let container_name = container_name.to_owned();
        let object_name = object_name.to_owned();
        let data = data.to_vec();
        await_owned_blob_mutation(mutation, async move {
            let namespace = BlobStorageNamespace::CustomStorage { environment_id };
            let path = Self::object_path(&container_name, &object_name)?;
            let blob_storage = storage.with("blob_store", "write_data");
            let old_size = blob_storage
                .get_metadata(namespace.clone(), &path)
                .await
                .map_err(blob_store_error)?
                .map_or(0, |metadata| metadata.size);
            let delta = signed_delta(data.len() as u64, old_size);
            if delta > 0 && !resource_limits.try_record_blob_storage_delta(delta) {
                return Err(BlobStoreError::LimitExceeded(
                    "Blob storage byte limit exceeded".to_string(),
                ));
            }
            if let Err(error) = blob_storage.put_raw(namespace, &path, &data).await {
                if delta > 0 {
                    resource_limits.rollback_blob_storage_delta(delta);
                }
                return Err(blob_store_error(error));
            }
            if delta < 0 {
                let recorded = resource_limits.try_record_blob_storage_delta(delta);
                debug_assert!(recorded);
            }
            Ok(BlobStoreMutation {
                bytes_delta: delta,
                objects_deleted: 0,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests;
