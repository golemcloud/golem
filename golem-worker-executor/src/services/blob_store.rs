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

use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::types::ObjectMetadata;
use golem_service_base::storage::blob::{
    BlobNameError, BlobRangeError, BlobStorage, BlobStorageNamespace, ExistsResult,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// Transient backend failure (network, timeout, etc.)
    TransientBackend(String),
    /// Other/unknown error
    Other(String),
}

impl std::fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::AlreadyExists(msg) => write!(f, "Already exists: {msg}"),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
            Self::TransientBackend(msg) => write!(f, "Backend error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Interface for storing blobs in a persistent storage.
#[async_trait]
pub trait BlobStoreService: Send + Sync {
    async fn clear(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError>;

    async fn copy_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn delete_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn delete_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<(), BlobStoreError>;

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

    async fn move_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError>;

    async fn write_data(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<(), BlobStoreError>;
}

pub struct DefaultBlobStoreService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

/// Gives the `BlobStoreError` of an error of the blob storage.
///
/// A [`BlobRangeError`] and a [`BlobNameError`] are errors of the input of the guest, so they
/// become [`BlobStoreError::InvalidInput`], which `classify_blob_store_error` in
/// `crate::durable_host::blobstore` makes permanent. Each other error becomes
/// [`BlobStoreError::TransientBackend`], which is transient, so the executor retries the
/// operation. Each method of [`DefaultBlobStoreService`] maps its errors with this function,
/// so an error of the input is permanent at each of them.
///
/// [`BlobNameError`] has one downcast here and one rule: each of its variants is a name that
/// the guest chose and that the storage cannot use, so each of them is permanent, whichever
/// backend gives it. The path rules are in it too, so a `..` name and an absolute name are
/// permanent like a name that S3 does not accept as an object key.
fn blob_store_error(err: anyhow::Error) -> BlobStoreError {
    if let Some(range) = err.downcast_ref::<BlobRangeError>() {
        BlobStoreError::InvalidInput(range.to_string())
    } else if let Some(name) = err.downcast_ref::<BlobNameError>() {
        BlobStoreError::InvalidInput(name.to_string())
    } else {
        BlobStoreError::TransientBackend(err.to_string())
    }
}

impl DefaultBlobStoreService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }
}

#[async_trait]
impl BlobStoreService for DefaultBlobStoreService {
    async fn clear(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Path::new(&container_name);
        self.blob_storage
            .delete_dir("blob_store", "clear", namespace.clone(), path)
            .await
            .map_err(blob_store_error)?;
        // Re-create the empty container directory so the container continues to exist.
        // clear() semantics: remove all objects, keep the container itself.
        self.blob_storage
            .create_dir("blob_store", "clear", namespace, path)
            .await
            .map_err(blob_store_error)?;
        Ok(())
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.blob_storage
            .exists(
                "blob_store",
                "container_exists",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
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
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .copy(
                "blob_store",
                "copy_object",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&source_container_name).join(&source_object_name),
                &Path::new(&destination_container_name).join(&destination_object_name),
            )
            .await
            .map_err(blob_store_error)
    }

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .create_dir(
                "blob_store",
                "create_container",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(blob_store_error)?;
        Ok(())
    }

    async fn delete_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .delete_dir(
                "blob_store",
                "delete_container",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(blob_store_error)?;
        Ok(())
    }

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .delete(
                "blob_store",
                "delete_object",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
            )
            .await
            .map_err(blob_store_error)?;
        Ok(())
    }

    async fn delete_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<(), BlobStoreError> {
        let paths: Vec<PathBuf> = object_names
            .iter()
            .map(|object_name| Path::new(container_name).join(object_name))
            .collect();
        self.blob_storage
            .delete_many(
                "blob_store",
                "delete_objects",
                BlobStorageNamespace::CustomStorage { environment_id },
                &paths,
            )
            .await
            .map_err(blob_store_error)
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        self.blob_storage
            .get_metadata(
                "blob_store",
                "get_container",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
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
        let data = self
            .blob_storage
            .get_raw_slice(
                "blob_store",
                "get_data",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
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
        self.blob_storage
            .exists(
                "blob_store",
                "has_object",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
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
        self.blob_storage
            .list_dir(
                "blob_store",
                "list_objects",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(blob_store_error)
            .map(|paths| {
                paths
                    .iter()
                    .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
                    .collect()
            })
    }

    async fn move_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .r#move(
                "blob_store",
                "move_object",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&source_container_name).join(&source_object_name),
                &Path::new(&destination_container_name).join(&destination_object_name),
            )
            .await
            .map_err(blob_store_error)
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        match self
            .blob_storage
            .get_metadata(
                "blob_store",
                "object_info",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
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
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .put_raw(
                "blob_store",
                "write_data",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(container_name).join(object_name),
                data,
            )
            .await
            .map_err(blob_store_error)
    }
}

#[cfg(test)]
mod tests {
    use crate::durable_host::blobstore::classify_blob_store_error;
    use crate::durable_host::durability::HostFailureKind;
    use crate::services::blob_store::{
        BlobStoreError, BlobStoreService, DefaultBlobStoreService, blob_store_error,
    };
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream::BoxStream;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::replayable_stream::ErasedReplayableStream;
    use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{
        BlobMetadata, BlobNameError, BlobRangeError, BlobStorage, BlobStorageNamespace,
        ExistsResult, ListedBlob,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tempfile::TempDir;
    use test_r::test;

    /// A blob storage that gives a `BlobNameError` for each operation.
    #[derive(Debug)]
    struct NameErrorBlobStorage;

    impl NameErrorBlobStorage {
        fn error<T>() -> Result<T, anyhow::Error> {
            Err(BlobNameError::NulByte.into())
        }
    }

    #[async_trait]
    impl BlobStorage for NameErrorBlobStorage {
        async fn get_raw(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<Option<Vec<u8>>, anyhow::Error> {
            Self::error()
        }

        async fn get_stream(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<Option<BoxStream<'static, Result<Bytes, anyhow::Error>>>, anyhow::Error>
        {
            Self::error()
        }

        async fn get_metadata(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<Option<BlobMetadata>, anyhow::Error> {
            Self::error()
        }

        async fn put_raw(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
            _data: &[u8],
        ) -> Result<(), anyhow::Error> {
            Self::error()
        }

        async fn put_stream(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
            _stream: &dyn ErasedReplayableStream<
                Item = Result<Vec<u8>, anyhow::Error>,
                Error = anyhow::Error,
            >,
        ) -> Result<(), anyhow::Error> {
            Self::error()
        }

        async fn delete(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<(), anyhow::Error> {
            Self::error()
        }

        async fn create_dir(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<(), anyhow::Error> {
            Self::error()
        }

        async fn list_dir(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<Vec<PathBuf>, anyhow::Error> {
            Self::error()
        }

        async fn list_blobs_below(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<Box<[ListedBlob]>, anyhow::Error> {
            Self::error()
        }

        async fn delete_dir(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<bool, anyhow::Error> {
            Self::error()
        }

        async fn exists(
            &self,
            _target_label: &'static str,
            _op_label: &'static str,
            _namespace: BlobStorageNamespace,
            _path: &Path,
        ) -> Result<ExistsResult, anyhow::Error> {
            Self::error()
        }
    }

    /// `blob_store_error` has one downcast for `BlobNameError`, so each rule of a name is
    /// permanent, and the rules of the path are in it with the rules of the object key of S3.
    ///
    /// The filesystem backend gives the two errors of the path, so the test reads the real
    /// rules and breaks if the type of their error changes. It also gives an error of the
    /// backend: a write that the filesystem cannot do. The NUL byte is a rule of the S3
    /// backend, and only a scripted transport gives it here, so the test builds that error.
    #[test]
    async fn blob_store_error_makes_an_error_of_the_input_permanent_and_a_backend_error_transient()
    {
        let tempdir = TempDir::new().unwrap();
        let storage = FileSystemBlobStorage::new(tempdir.path()).await.unwrap();
        let namespace = BlobStorageNamespace::CustomStorage {
            environment_id: EnvironmentId::new(),
        };
        let put = |path: &'static str| {
            let namespace = namespace.clone();
            let storage = &storage;
            async move {
                storage
                    .put_raw("test", "put-raw", namespace, Path::new(path), &[1])
                    .await
            }
        };

        // A blob at `file` makes `file/blob` a path that the filesystem cannot write, because
        // the parent of the blob is a file and not a directory.
        put("file").await.unwrap();
        let backend = blob_store_error(put("file/blob").await.unwrap_err());
        let names = [
            put("../escape").await.unwrap_err(),
            put("/escape").await.unwrap_err(),
            BlobNameError::NulByte.into(),
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

    #[test]
    async fn every_operation_gives_invalid_input_for_a_name_error() {
        let blob_store = DefaultBlobStoreService::new(Arc::new(NameErrorBlobStorage));
        let environment_id = EnvironmentId::new();
        let container = || "container".to_string();
        let object = || "object".to_string();

        let errors: Vec<Result<(), BlobStoreError>> = vec![
            blob_store.clear(environment_id, container()).await,
            blob_store
                .container_exists(environment_id, container())
                .await
                .map(drop),
            blob_store
                .copy_object(environment_id, container(), object(), container(), object())
                .await,
            blob_store
                .create_container(environment_id, container())
                .await,
            blob_store
                .delete_container(environment_id, container())
                .await,
            blob_store
                .delete_object(environment_id, container(), object())
                .await,
            blob_store
                .delete_objects(environment_id, "container", &[object()])
                .await,
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
                .move_object(environment_id, container(), object(), container(), object())
                .await,
            blob_store
                .object_info(environment_id, container(), object())
                .await
                .map(drop),
            blob_store
                .write_data(environment_id, "container", "object", &[1])
                .await,
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
            .delete_container(environment_id, "container1".to_string())
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
            .write_data(environment_id, "container1", "obj1", &original_data)
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
            .write_data(environment_id, "container1", "obj1", &original_data)
            .await
            .unwrap();

        blob_store
            .copy_object(
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
            .write_data(environment_id, "container1", "obj1", &[1, 2, 3, 4])
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
            .write_data(environment_id, "container1", "../../escape", &[1])
            .await;
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
            .await;

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

    fn in_memory_blob_store() -> impl BlobStoreService {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        DefaultBlobStoreService::new(blob_storage)
    }

    async fn fs_blob_store(path: &Path) -> impl BlobStoreService {
        let blob_storage = Arc::new(FileSystemBlobStorage::new(path).await.unwrap());
        DefaultBlobStoreService::new(blob_storage)
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
}
