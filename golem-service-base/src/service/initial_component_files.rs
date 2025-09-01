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

use std::{path::PathBuf, sync::Arc};

use crate::replayable_stream::{ContentHash, ReplayableStream};
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::{InitialComponentFileKey, ProjectId};
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
        project_id: &ProjectId,
        key: &InitialComponentFileKey,
    ) -> Result<bool, String> {
        let path = PathBuf::from(key.0.clone());

        let metadata = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "exists",
                BlobStorageNamespace::InitialComponentFiles {
                    project_id: project_id.clone(),
                },
                &path,
            )
            .await
            .map_err(|err| format!("Failed to get metadata: {err}"))?;

        Ok(metadata.is_some())
    }

    pub async fn get(
        &self,
        project_id: &ProjectId,
        key: &InitialComponentFileKey,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, String>>>, String> {
        self.blob_storage
            .get_stream(
                INITIAL_COMPONENT_FILES_LABEL,
                "get",
                BlobStorageNamespace::InitialComponentFiles {
                    project_id: project_id.clone(),
                },
                &PathBuf::from(key.0.clone()),
            )
            .await
    }

    pub async fn put_if_not_exists(
        &self,
        project_id: &ProjectId,
        data: impl ReplayableStream<Item = Result<Bytes, String>, Error = String>,
    ) -> Result<InitialComponentFileKey, String> {
        let hash = data.content_hash().await?;

        let key = PathBuf::from(hash.clone());

        let metadata = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "get_metadata",
                BlobStorageNamespace::InitialComponentFiles {
                    project_id: project_id.clone(),
                },
                &key,
            )
            .await
            .map_err(|err| format!("Failed to get metadata: {err}"))?;

        if metadata.is_none() {
            debug!("Storing initial component file with hash: {}", hash);

            self.blob_storage
                .put_stream(
                    INITIAL_COMPONENT_FILES_LABEL,
                    "put",
                    BlobStorageNamespace::InitialComponentFiles {
                        project_id: project_id.clone(),
                    },
                    &key,
                    &data.erased(),
                )
                .await?;
        };
        Ok(InitialComponentFileKey(hash))
    }
}
