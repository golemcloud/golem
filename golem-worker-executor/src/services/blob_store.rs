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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::Mutex;

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
    mutation_locks: StdMutex<HashMap<EnvironmentId, Weak<Mutex<()>>>>,
}

/// Gives the `BlobStoreError` of an error of the blob storage.
///
/// [`BlobFailure::of`] tells a permanent error from a transient one. A transient error becomes
/// [`BlobStoreError::TransientBackend`], so the executor retries the operation. Of the permanent
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
            mutation_locks: StdMutex::new(HashMap::new()),
        }
    }

    fn mutation_lock(&self, environment_id: EnvironmentId) -> Arc<Mutex<()>> {
        let mut locks = self.mutation_locks.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&environment_id).and_then(Weak::upgrade) {
            return lock;
        }

        let lock = Arc::new(Mutex::new(()));
        locks.insert(environment_id, Arc::downgrade(&lock));
        lock
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
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::container_path(&container_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "clear");
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
        if !blob_storage
            .delete_dir(namespace.clone(), path)
            .await
            .map_err(blob_store_error)?
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
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let source = Self::object_path(&source_container_name, &source_object_name)?;
        let destination = Self::object_path(&destination_container_name, &destination_object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "copy_object");
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
    }

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "create_container")
            .create_dir(BlobStorageNamespace::CustomStorage { environment_id }, path)
            .await
            .map_err(blob_store_error)?;
        Ok(BlobStoreMutation::default())
    }

    async fn delete_container(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::container_path(&container_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "delete_container");
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
        if !blob_storage
            .delete_dir(namespace, path)
            .await
            .map_err(blob_store_error)?
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
    }

    async fn delete_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::object_path(&container_name, &object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "delete_object");
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
    }

    async fn delete_objects(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        Self::container_path(container_name)?;
        let paths: HashMap<String, PathBuf> = object_names
            .iter()
            .map(|object_name| {
                let path = Self::object_path(container_name, object_name)?;
                Ok((Self::blob_path_identity(&path)?, path))
            })
            .collect::<Result<_, _>>()?;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let mut deleted_bytes = 0u64;
        let mut existing_paths = Vec::new();
        let blob_storage = self.blob_storage.with("blob_store", "delete_objects");
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
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let source = Self::object_path(&source_container_name, &source_object_name)?;
        let destination = Self::object_path(&destination_container_name, &destination_object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "move_object");
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
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::object_path(container_name, object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "write_data");
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
        if let Err(error) = blob_storage.put_raw(namespace, &path, data).await {
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
    }
}

#[cfg(test)]
mod tests {
    use crate::durable_host::blobstore::classify_blob_store_error;
    use crate::durable_host::durability::HostFailureKind;
    use crate::services::blob_store::{
        BlobStoreError, BlobStoreMutation, BlobStoreService, DefaultBlobStoreService,
        blob_store_error,
    };
    use crate::services::resource_limits::AtomicResourceEntry;
    use anyhow::Error;
    use async_trait::async_trait;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::db::sqlite::SqlitePool;
    use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
    use golem_service_base::storage::blob::{
        BlobMetadata, BlobMissingError, BlobNameError, BlobRangeError, BlobRangeStream,
        BlobStorage, BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob,
        NormalizedBlobPath, PutIfAbsent,
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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
    /// `put_raw`.
    #[derive(Debug)]
    struct FailingPutBlobStorage {
        inner: InMemoryBlobStorage,
        fail_next_put: AtomicBool,
        block_next_put: AtomicBool,
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
            if self.fail_next_put.swap(false, Ordering::AcqRel) {
                return Err(anyhow::anyhow!("injected put failure"));
            }
            if self.block_next_put.swap(false, Ordering::AcqRel) {
                self.put_started.notify_one();
                self.release_put.notified().await;
            }
            self.inner
                .put_raw_at(target_label, op_label, namespace, path, data)
                .await
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
        ) -> Result<bool, Error> {
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
        }
    }

    /// `BlobFailure::of` has one downcast for `BlobNameError`, so each rule of a name is
    /// permanent, and the rules of the path are in it with the rules of the object key of S3.
    ///
    /// The filesystem backend gives the three errors of the path, so the test reads the real
    /// rules and breaks if the type of their error changes. The NUL rule is a rule of MinIO,
    /// and every backend applies it, so the filesystem backend gives it here too. The backend
    /// also gives an error of its own: a write of a path that is too long for the filesystem.
    #[test]
    async fn blob_store_error_makes_an_error_of_the_input_permanent_and_a_backend_error_transient()
    {
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

        let outside = futures::future::join_all(
            [(0, 4), (4, 4), (2, 1)].map(|(start, end)| read(start, end)),
        )
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
    async fn test_root_container_mutations_do_not_release_quota(
        blob_store: &impl BlobStoreService,
    ) {
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
        test_root_container_mutations_do_not_release_quota(&fs_blob_store(tempdir.path()).await)
            .await;
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
}
