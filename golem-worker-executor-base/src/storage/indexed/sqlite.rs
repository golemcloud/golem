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

use async_trait::async_trait;
use bytes::Bytes;
use futures::TryFutureExt;
use std::time::Duration;

use crate::storage::sqlite_types::SqlitePool;

use super::{IndexedStorage, IndexedStorageNamespace, ScanCursor};

#[derive(Debug)]
pub struct SqliteIndexedStorage {
    pool: SqlitePool,
}

impl SqliteIndexedStorage {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn to_string(namespace: &IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog => "worker-oplog".to_string(),
            IndexedStorageNamespace::CompressedOpLog { level } => {
                format!("worker-c{level}-oplog")
            }
        }
    }
}

#[async_trait]
impl IndexedStorage for SqliteIndexedStorage {
    async fn number_of_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
    ) -> Result<u8, String> {
        Ok(1)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, String> {
        Ok(1)
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.pool
            .with(svc_name, api_name)
            .exists_index(&Self::to_string(&namespace), key)
            .map_err(|e| e.to_string())
            .await
    }

    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        self.pool
            .with(svc_name, api_name)
            .scan(&Self::to_string(&namespace), pattern, cursor, count)
            .map_err(|e| e.to_string())
            .await
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .append(&Self::to_string(&namespace), key, id, value)
            .map_err(|e| e.to_string())
            .await
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
        self.pool
            .with(svc_name, api_name)
            .length(&Self::to_string(&namespace), key)
            .map_err(|e| e.to_string())
            .await
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .delete(&Self::to_string(&namespace), key)
            .map_err(|e| e.to_string())
            .await
    }

    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Bytes)>, String> {
        self.pool
            .with(svc_name, api_name)
            .read(&Self::to_string(&namespace), key, start_id, end_id)
            .map_err(|e| e.to_string())
            .await
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
        self.pool
            .with(svc_name, api_name)
            .first(&Self::to_string(&namespace), key)
            .map_err(|e| e.to_string())
            .await
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
        self.pool
            .with(svc_name, api_name)
            .last(&Self::to_string(&namespace), key)
            .map_err(|e| e.to_string())
            .await
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Bytes)>, String> {
        self.pool
            .with(svc_name, api_name)
            .closest(&Self::to_string(&namespace), key, id)
            .map_err(|e| e.to_string())
            .await
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .drop_prefix(&Self::to_string(&namespace), key, last_dropped_id)
            .map_err(|e| e.to_string())
            .await
    }
}
