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

use crate::replayable_stream::ErasedReplayableStream;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::{AccountId, ComponentId, Timestamp, WorkerId};
use golem_common::serialization::{deserialize, serialize};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

pub mod fs;
pub mod memory;
pub mod s3;
pub mod sqlite;

#[async_trait]
pub trait BlobStorage: Debug {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Bytes>, String>;

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, String>>>, String>;

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Bytes>, String> {
        let data = self
            .get_raw(target_label, op_label, namespace, path)
            .await?;
        Ok(data.map(|data| data.slice((start as usize)..(end as usize))))
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String>;

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String>;

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Bytes, String>, Error = String>,
    ) -> Result<(), String>;

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String>;

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        for path in paths {
            self.delete(target_label, op_label, namespace.clone(), path)
                .await?;
        }
        Ok(())
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String>;

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String>;

    /// Returns true if the directory was deleted; false if it did not exist
    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, String>;

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String>;

    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), String> {
        match self
            .get_raw(target_label, op_label, namespace.clone(), from)
            .await?
        {
            Some(data) => {
                self.put_raw(target_label, op_label, namespace, to, &data)
                    .await
            }
            None => Err(format!("Entry not found: {:?}", from)),
        }
    }

    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), String> {
        self.copy(target_label, op_label, namespace.clone(), from, to)
            .await?;
        self.delete(target_label, op_label, namespace, from).await
    }
}

pub trait BlobStorageLabelledApi<S: BlobStorage + ?Sized + Sync> {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledBlobStorage<S>;
}

impl<S: BlobStorage + ?Sized + Sync> BlobStorageLabelledApi<S> for S {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledBlobStorage<Self> {
        LabelledBlobStorage::new(svc_name, api_name, self)
    }
}

pub struct LabelledBlobStorage<'a, S: BlobStorage + ?Sized + Sync> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: BlobStorage + ?Sized + Sync> LabelledBlobStorage<'a, S> {
    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn get_raw(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Bytes>, String> {
        self.storage
            .get_raw(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn get_raw_slice(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Bytes>, String> {
        self.storage
            .get_raw_slice(self.svc_name, self.api_name, namespace, path, start, end)
            .await
    }

    pub async fn get_metadata(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        self.storage
            .get_metadata(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn put_raw(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        self.storage
            .put_raw(self.svc_name, self.api_name, namespace, path, data)
            .await
    }

    pub async fn delete(&self, namespace: BlobStorageNamespace, path: &Path) -> Result<(), String> {
        self.storage
            .delete(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn delete_many(
        &self,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        self.storage
            .delete_many(self.svc_name, self.api_name, namespace, paths)
            .await
    }

    pub async fn create_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        self.storage
            .create_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn list_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        self.storage
            .list_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn delete_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, String> {
        self.storage
            .delete_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn exists(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn copy(
        &self,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), String> {
        self.storage
            .copy(self.svc_name, self.api_name, namespace, from, to)
            .await
    }

    pub async fn r#move(
        &self,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), String> {
        self.storage
            .r#move(self.svc_name, self.api_name, namespace, from, to)
            .await
    }

    pub async fn get<T: Decode>(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<T>, String> {
        match self.get_raw(namespace, path).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    pub async fn put<T: Encode>(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &T,
    ) -> Result<(), String> {
        self.put_raw(namespace, path, &serialize(data)?).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlobStorageNamespace {
    CompilationCache,
    InitialComponentFiles {
        account_id: AccountId,
    },
    CustomStorage(AccountId),
    OplogPayload {
        account_id: AccountId,
        worker_id: WorkerId,
    },
    CompressedOplog {
        account_id: AccountId,
        component_id: ComponentId,
        level: usize,
    },
    // TODO: prefix with account_id and move existing data
    Components,
    PluginWasmFiles {
        account_id: AccountId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistsResult {
    File,
    Directory,
    DoesNotExist,
}

#[derive(Debug, Clone)]
pub struct BlobMetadata {
    pub last_modified_at: Timestamp,
    pub size: u64,
}
