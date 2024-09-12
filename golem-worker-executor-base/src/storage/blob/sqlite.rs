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

use std::path::{Path, PathBuf};

use crate::storage::{
    blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult},
    sqlite_types::SqlitePool,
};
use async_trait::async_trait;
use bytes::Bytes;

#[derive(Debug)]
pub struct SqliteBlobStorage {
    pool: SqlitePool,
}

impl SqliteBlobStorage {
    pub fn new(pool: SqlitePool) -> Self {
        SqliteBlobStorage { pool }
    }

    fn into_string(namespace: BlobStorageNamespace) -> String {
        match namespace {
            BlobStorageNamespace::CompilationCache => "compilation_cache".to_string(),
            BlobStorageNamespace::CustomStorage(account_id) => {
                format!("custom_data-{}", account_id.value)
            }
            BlobStorageNamespace::OplogPayload {
                account_id,
                worker_id,
            } => format!(
                "oplog_payload-{}-{}",
                account_id.value, worker_id.worker_name
            ),
            BlobStorageNamespace::CompressedOplog {
                account_id,
                component_id,
                level,
            } => format!(
                "compressed_oplog-{}-{}-{}",
                account_id.value, component_id, level
            ),
        }
    }

    fn to_string(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }
}

#[async_trait]
impl BlobStorage for SqliteBlobStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Bytes>, String> {
        self.pool
            .with(target_label, op_label)
            .get_raw(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        self.pool
            .with(target_label, op_label)
            .get_metadata(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(target_label, op_label)
            .put_raw(&Self::into_string(namespace), &Self::to_string(path), data)
            .await
            .map_err(|err| err.to_string())
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        self.pool
            .with(target_label, op_label)
            .delete_blob(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        self.pool
            .with(target_label, op_label)
            .create_dir(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        self.pool
            .with(target_label, op_label)
            .list_dir(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        self.pool
            .with(target_label, op_label)
            .delete_dir(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        self.pool
            .with(target_label, op_label)
            .exists_blob(&Self::into_string(namespace), &Self::to_string(path))
            .await
            .map_err(|err| err.to_string())
    }
}
