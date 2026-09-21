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

use super::ErasedReplayableStream;
use crate::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath,
    PutIfAbsent, blob_child_path, normalized_blob_path,
};
use anyhow::Error;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, TryStreamExt};
use golem_common::model::Timestamp;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    pin::Pin,
};

/// The place of one blob, or of one directory that `create_dir` made.
///
/// A blob has the path of the directory that holds it and its own name. A directory has its own
/// path and no name. A directory that only holds blobs has no key, because the keys of the blobs
/// below it tell that it is there. S3 keeps the same record, where a key is an object key and a
/// directory of `create_dir` is a marker object, so the two backends give the same answers.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct Key {
    namespace: BlobStorageNamespace,
    dir: String,
    file: Option<String>,
}

#[derive(Debug, Clone)]
enum Entry {
    /// A directory that `create_dir` made, with the time of the last such call.
    Directory { created_at: Timestamp },
    File {
        data: Vec<u8>,
        metadata: BlobMetadata,
    },
}

#[derive(Debug)]
pub struct InMemoryBlobStorage {
    data: scc::HashMap<Key, Entry>,
}

impl Default for InMemoryBlobStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBlobStorage {
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::new(),
        }
    }

    /// Gives the key of the blob at the path.
    ///
    /// The path is not at the root of the namespace.
    fn blob_key(namespace: BlobStorageNamespace, path: &NormalizedBlobPath) -> Result<Key, Error> {
        Ok(Key {
            namespace,
            dir: path.parent_text()?,
            file: Some(path.file_name_text()?),
        })
    }

    /// Tells if the key is in the directory, or below it at any depth.
    ///
    /// The directory is not at the root of the namespace. A blob at the path of the directory is
    /// not in it, because the directory of that blob is the one above.
    fn is_in_dir(key: &Key, namespace: &BlobStorageNamespace, dir: &str) -> bool {
        key.namespace == *namespace
            && (key.dir == dir
                || key
                    .dir
                    .strip_prefix(dir)
                    .is_some_and(|below| below.starts_with('/')))
    }
}

#[async_trait]
impl BlobStorage for InMemoryBlobStorage {
    async fn get_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error> {
        let path = normalized_blob_path(path)?;

        // A root path is a directory, and a directory has no blob at its path.
        if path.is_root() {
            return Ok(None);
        }

        let key = Self::blob_key(namespace, &path)?;

        Ok(self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::File { data, .. } => Some(data.clone()),
                Entry::Directory { .. } => None,
            })
            .await
            .flatten())
    }

    async fn get_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        let path = normalized_blob_path(path)?;

        // A root path is a directory, and a directory has no blob at its path.
        if path.is_root() {
            return Ok(None);
        }

        let key = Self::blob_key(namespace, &path)?;

        Ok(self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::File { data, .. } => {
                    let stream = tokio_stream::once(Ok(Bytes::from(data.clone())));
                    let boxed: Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>> =
                        Box::pin(stream);
                    Some(boxed)
                }
                Entry::Directory { .. } => None,
            })
            .await
            .flatten())
    }

    async fn get_metadata(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        let path = normalized_blob_path(path)?;

        // A root path is a directory, and create_dir leaves no directory at the root.
        if path.is_root() {
            return Ok(None);
        }

        let blob_key = Self::blob_key(namespace.clone(), &path)?;
        let blob_metadata = self
            .data
            .read_async(&blob_key, |_, entry| match entry {
                Entry::File { metadata, .. } => Some(metadata.clone()),
                Entry::Directory { .. } => None,
            })
            .await
            .flatten();

        if blob_metadata.is_some() {
            return Ok(blob_metadata);
        }

        // A directory that create_dir made has a size of zero and the time of that call. A
        // directory that only holds blobs has no key, so it has no time of its own.
        let dir_key = Key {
            namespace,
            dir: path.text()?,
            file: None,
        };

        Ok(self
            .data
            .read_async(&dir_key, |_, entry| match entry {
                Entry::Directory { created_at } => Some(BlobMetadata {
                    size: 0,
                    last_modified_at: *created_at,
                }),
                Entry::File { .. } => None,
            })
            .await
            .flatten())
    }

    async fn put_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error> {
        let path = normalized_blob_path(path)?;
        path.reject_root()?;

        let key = Self::blob_key(namespace, &path)?;
        let entry = Entry::File {
            data: data.to_vec(),
            metadata: BlobMetadata {
                size: data.len() as u64,
                last_modified_at: Timestamp::now_utc(),
            },
        };

        self.data.upsert_async(key, entry).await;

        Ok(())
    }

    async fn put_raw_if_absent(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        let path = normalized_blob_path(path)?;
        path.reject_root()?;

        // A directory that `create_dir` made has a key without a file name, so only a blob at
        // the path holds this key. The insert refuses a key that is there, in one step.
        let key = Self::blob_key(namespace, &path)?;
        let entry = Entry::File {
            data: data.to_vec(),
            metadata: BlobMetadata {
                size: data.len() as u64,
                last_modified_at: Timestamp::now_utc(),
            },
        };

        Ok(match self.data.insert_async(key, entry).await {
            Ok(()) => PutIfAbsent::Written,
            Err(_) => PutIfAbsent::AlreadyExists,
        })
    }

    async fn put_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let path = normalized_blob_path(path)?;
        path.reject_root()?;

        let key = Self::blob_key(namespace, &path)?;

        let stream = stream.make_stream_erased().await?;
        let data = stream.try_collect::<Vec<_>>().await?.concat();
        let entry = Entry::File {
            metadata: BlobMetadata {
                size: data.len() as u64,
                last_modified_at: Timestamp::now_utc(),
            },
            data,
        };

        self.data.upsert_async(key, entry).await;

        Ok(())
    }

    async fn delete(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        let path = normalized_blob_path(path)?;

        // A root path is a directory, and a directory has no blob at its path to remove.
        if path.is_root() {
            return Ok(());
        }

        let key = Self::blob_key(namespace, &path)?;
        self.data.remove_async(&key).await;

        Ok(())
    }

    async fn create_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        let path = normalized_blob_path(path)?;

        if path.is_root() {
            return Ok(());
        }

        let key = Key {
            namespace,
            dir: path.text()?,
            file: None,
        };

        self.data
            .upsert_async(
                key,
                Entry::Directory {
                    created_at: Timestamp::now_utc(),
                },
            )
            .await;

        Ok(())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        let path = normalized_blob_path(path)?;
        let dir = path.text()?;

        // The directory holds the blobs that have it as their directory. It also holds the
        // directories that `create_dir` made below it, at the depth of their own key.
        let nested = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };

        // A blob and a directory can hold one path, and then the storage has a key for the blob
        // and a key for the directory. The set gives the path one time.
        let mut entries = HashSet::new();
        self.data
            .iter_async(|key, _| {
                if key.namespace == namespace {
                    if key.dir == dir {
                        if let Some(file) = &key.file {
                            entries.insert(path.join(file));
                        }
                    } else if key.file.is_none() && key.dir.starts_with(&nested) {
                        entries.insert(PathBuf::from(&key.dir));
                    }
                }
                true
            })
            .await;

        Ok(entries.into_iter().collect())
    }

    async fn list_blobs_below(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        let path = normalized_blob_path(path)?;
        let directory = path.text()?;
        let nested = format!("{directory}/");

        let mut listed = Vec::new();
        self.data
            .iter_async(|key, entry| {
                if let (Some(name), Entry::File { metadata, .. }) = (&key.file, entry)
                    && key.namespace == namespace
                    && (directory.is_empty()
                        || key.dir == directory
                        || key.dir.starts_with(&nested))
                {
                    listed.push(ListedBlob {
                        path: blob_child_path(&key.dir, name),
                        size: metadata.size,
                    });
                }
                true
            })
            .await;
        Ok(listed.into_boxed_slice())
    }

    async fn delete_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        let path = normalized_blob_path(path)?;

        if path.is_root() {
            return Ok(false);
        }

        let dir = path.text()?;

        // A directory that only holds blobs has no key of its own, so the keys below it tell
        // that it is there. They go with it, at any depth.
        let mut deleted = false;
        self.data
            .retain_async(|key, _| {
                if Self::is_in_dir(key, &namespace, &dir) {
                    deleted = true;
                    false
                } else {
                    true
                }
            })
            .await;

        Ok(deleted)
    }

    async fn exists(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        let path = normalized_blob_path(path)?;

        // The root of a namespace is a directory, also when the namespace holds nothing.
        if path.is_root() {
            return Ok(ExistsResult::Directory);
        }

        // A blob at the path wins over a directory at the same path, because the other backends
        // answer that way: a key that holds bytes is a file, whatever sits below it.
        let blob_key = Self::blob_key(namespace.clone(), &path)?;
        if self.data.contains_async(&blob_key).await {
            return Ok(ExistsResult::File);
        }

        // A directory that only holds blobs has no key of its own, so the keys below it tell
        // that it is there. This is the range that delete_dir removes.
        let dir = path.text()?;
        let has_keys_below = self
            .data
            .any_async(|key, _| Self::is_in_dir(key, &namespace, &dir))
            .await
            .is_some();

        if has_keys_below {
            Ok(ExistsResult::Directory)
        } else {
            Ok(ExistsResult::DoesNotExist)
        }
    }
}
