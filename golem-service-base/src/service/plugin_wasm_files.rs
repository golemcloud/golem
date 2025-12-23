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
use anyhow::{Context, Error};
use golem_common::model::account::AccountId;
use golem_common::model::plugin_registration::WasmContentHash;
use tracing::debug;

const PLUGIN_WASM_FILES_LABEL: &str = "plugin_wasms";

/// Service for storing plugin wasm files.
#[derive(Debug)]
pub struct PluginWasmFilesService {
    blob_storage: Arc<dyn BlobStorage>,
}

impl PluginWasmFilesService {
    pub fn new(blob_storage: Arc<dyn BlobStorage>) -> Self {
        Self { blob_storage }
    }

    pub async fn get(
        &self,
        account_id: AccountId,
        key: &WasmContentHash,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.blob_storage
            .get_raw(
                PLUGIN_WASM_FILES_LABEL,
                "get",
                BlobStorageNamespace::PluginWasmFiles { account_id },
                &PathBuf::from(key.0.into_blake3().to_hex().to_string()),
            )
            .await
    }

    pub async fn put_if_not_exists(
        &self,
        account_id: AccountId,
        data: impl ReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<WasmContentHash, Error> {
        let hash = data.content_hash().await?;
        let key = PathBuf::from(hash.into_blake3().to_hex().to_string());

        let metadata = self
            .blob_storage
            .get_metadata(
                PLUGIN_WASM_FILES_LABEL,
                "get_metadata",
                BlobStorageNamespace::PluginWasmFiles { account_id },
                &key,
            )
            .await
            .context("Failed to get metadata")?;

        if metadata.is_none() {
            debug!("Storing library plugin file with hash: {}", hash);

            self.blob_storage
                .put_stream(
                    PLUGIN_WASM_FILES_LABEL,
                    "put",
                    BlobStorageNamespace::PluginWasmFiles { account_id },
                    &key,
                    &data.erased(),
                )
                .await?;
        };
        Ok(WasmContentHash(hash))
    }
}
