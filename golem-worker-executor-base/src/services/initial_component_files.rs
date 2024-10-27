// Copyright 2024 Golem Cloud
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

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;

use crate::storage::blob::{BlobStorage, BlobStorageNamespace};
use golem_common::model::InitialComponentFileKey;

const INITIAL_COMPONENT_FILES_LABEL: &str = "initial_component_files";

/// Service for storing initial component files.
#[async_trait]
pub trait InitialComponentFilesService {
    async fn get(
        &self,
        key: &InitialComponentFileKey,
    ) -> Result<Option<Bytes>, String>;
    async fn put_if_not_exists(
        &self,
        key: &InitialComponentFileKey,
        bytes: Bytes,
    ) -> Result<(), String>;
}

pub struct InitialComponentFilesServiceDefault {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl InitialComponentFilesServiceDefault {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }
}

#[async_trait]
impl InitialComponentFilesService for InitialComponentFilesServiceDefault {
    async fn get(
        &self,
        key: &InitialComponentFileKey,
    ) -> Result<Option<Bytes>, String> {
        self
            .blob_storage
            .get_raw(
                INITIAL_COMPONENT_FILES_LABEL,
                "get",
                BlobStorageNamespace::InitialComponentFiles,
                &PathBuf::from(key.0.clone()),
            )
            .await
    }

    async fn put_if_not_exists(
        &self,
        key: &InitialComponentFileKey,
        bytes: Bytes,
    ) -> Result<(), String> {
        let path = PathBuf::from(key.0.clone());

        let exists = self
            .blob_storage
            .get_metadata(
                INITIAL_COMPONENT_FILES_LABEL,
                "put",
                BlobStorageNamespace::InitialComponentFiles,
                &path,
            )
            .await
            .map_err(|err| format!("Failed to get metadata: {}", err))?;

        if !exists.is_some() {
            self.blob_storage
                .put_raw(
                    INITIAL_COMPONENT_FILES_LABEL,
                    "put",
                    BlobStorageNamespace::InitialComponentFiles,
                    &path,
                    &bytes,
                )
                .await
        } else {
            Ok(())
        }
    }
}
