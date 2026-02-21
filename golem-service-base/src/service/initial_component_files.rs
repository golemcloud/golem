// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::replayable_stream::{ContentHash, ReplayableStream};
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};
use anyhow::{Context, Error};
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::component::ComponentFileContentHash;
use golem_common::model::environment::EnvironmentId;
use std::{path::PathBuf, sync::Arc};
use tracing::debug;

const INITIAL_COMPONENT_FILES_LABEL: &str = "initial_component_files";

/// Service for storing initial component files.
#[derive(Debug)]
pub struct InitialComponentFilesService {
    blob_storage: Arc<dyn BlobStorage>,
}

impl InitialComponentFilesService {
    pub fn new(blob_storage: Arc<dyn BlobStorage>) -> Self {
        Self { blob_storage }
    }

    pub async fn exists(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
    ) -> Result<bool, Error> {
        let metadata = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "exists",
                BlobStorageNamespace::InitialComponentFiles { environment_id },
                &PathBuf::from(key.0.into_blake3().to_hex().to_string()),
            )
            .await
            .context("Failed getting metadata")?;

        Ok(metadata.is_some())
    }

    pub async fn get(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        self.blob_storage
            .get_stream(
                INITIAL_COMPONENT_FILES_LABEL,
                "get",
                BlobStorageNamespace::InitialComponentFiles { environment_id },
                &PathBuf::from(key.0.into_blake3().to_hex().to_string()),
            )
            .await
            .context("Failed getting data stream")
    }

    pub async fn put_if_not_exists(
        &self,
        environment_id: EnvironmentId,
        data: impl ReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<ComponentFileContentHash, Error> {
        let hash = data.content_hash().await?;
        let key = PathBuf::from(hash.into_blake3().to_hex().to_string());

        let metadata = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "get_metadata",
                BlobStorageNamespace::InitialComponentFiles { environment_id },
                &key,
            )
            .await
            .context("Failed getting metadata")?;

        if metadata.is_none() {
            debug!("Storing initial component file with hash: {}", hash);

            self.blob_storage
                .put_stream(
                    INITIAL_COMPONENT_FILES_LABEL,
                    "put",
                    BlobStorageNamespace::InitialComponentFiles { environment_id },
                    &key,
                    &data.erased(),
                )
                .await
                .context("Failed storing blob storage data")?;
        };
        Ok(ComponentFileContentHash(hash))
    }
}
