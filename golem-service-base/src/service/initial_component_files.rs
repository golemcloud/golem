// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{path::PathBuf, pin::Pin, sync::Arc};

use crate::storage::blob::{BlobStorage, BlobStorageNamespace, ReplayableStream};
use bytes::Bytes;
use futures::TryStreamExt;
use golem_common::model::{AccountId, InitialComponentFileKey};
use tracing::debug;

const INITIAL_COMPONENT_FILES_LABEL: &str = "initial_component_files";

/// Service for storing initial component files.
pub struct InitialComponentFilesService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl InitialComponentFilesService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }

    pub async fn exists(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
    ) -> Result<bool, String> {
        let path = PathBuf::from(key.0.clone());

        let metadata = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "exists",
                BlobStorageNamespace::InitialComponentFiles {
                    account_id: account_id.clone(),
                },
                &path,
            )
            .await
            .map_err(|err| format!("Failed to get metadata: {}", err))?;

        Ok(metadata.is_some())
    }

    pub async fn get(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
    ) -> Result<Option<Pin<Box<dyn futures::Stream<Item = Result<Bytes, String>> + Send>>>, String>
    {
        self.blob_storage
            .get_stream(
                INITIAL_COMPONENT_FILES_LABEL,
                "get",
                BlobStorageNamespace::InitialComponentFiles {
                    account_id: account_id.clone(),
                },
                &PathBuf::from(key.0.clone()),
            )
            .await
    }

    pub async fn put_if_not_exists(
        &self,
        account_id: &AccountId,
        data: &impl ReplayableStream<Item = Result<Bytes, String>>,
    ) -> Result<InitialComponentFileKey, String> {
        let hash = content_hash_stream(data)
            .await
            .map_err(|err| format!("Failed to hash content: {}", err))?;

        let key = PathBuf::from(hash.clone());

        let metadata = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "put",
                BlobStorageNamespace::InitialComponentFiles {
                    account_id: account_id.clone(),
                },
                &key,
            )
            .await
            .map_err(|err| format!("Failed to get metadata: {}", err))?;

        if metadata.is_none() {
            debug!("Storing initial component file with hash: {}", hash);

            self.blob_storage
                .put_stream(
                    INITIAL_COMPONENT_FILES_LABEL,
                    "put",
                    BlobStorageNamespace::InitialComponentFiles {
                        account_id: account_id.clone(),
                    },
                    &key,
                    data,
                )
                .await?;
        };
        Ok(InitialComponentFileKey(hash))
    }
}

async fn content_hash_stream(
    stream: &impl ReplayableStream<Item = Result<Bytes, String>>,
) -> Result<String, HashingError> {
    let stream = stream.make_stream().await.map_err(HashingError)?;
    let stream = stream.map_ok(|b| b.to_vec()).map_err(HashingError);
    let hash = async_hash::hash_try_stream::<async_hash::Sha256, _, _, _>(stream).await?;
    Ok(hex::encode(hash))
}

#[derive(Debug, Clone)]
struct HashingError(String);

impl std::fmt::Display for HashingError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Hashing error: {}", self.0)
    }
}

impl std::error::Error for HashingError {}
