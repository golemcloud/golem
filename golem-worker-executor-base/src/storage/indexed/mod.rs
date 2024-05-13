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

pub mod memory;
pub mod redis;

use std::fmt::Debug;
use std::time::Duration;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::serialization::{deserialize, serialize};

type ScanCursor = u64;

// TODO: review namespace parameter, possibly make it non-Option? Maybe non-string?
#[async_trait]
pub trait IndexedStorage: Debug {
    async fn number_of_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> Result<u8, String>;

    async fn wait_for_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, String>;

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<bool, String>;

    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String>;

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: Option<&str>,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), String>;

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<u64, String>;

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<(), String>;

    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: Option<&str>,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<Bytes>, String>;
}

pub trait IndexedStorageLabelledApi<T: IndexedStorage + ?Sized> {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledIndexedStorage<T>;

    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityIndexedStorage<T>;
}

impl<T: ?Sized + IndexedStorage> IndexedStorageLabelledApi<T> for T {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledIndexedStorage<T> {
        LabelledIndexedStorage::new(svc_name, api_name, self)
    }
    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityIndexedStorage<T> {
        LabelledEntityIndexedStorage::new(svc_name, api_name, entity_name, self)
    }
}

pub struct LabelledIndexedStorage<'a, S: IndexedStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + IndexedStorage> LabelledIndexedStorage<'a, S> {
    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn number_of_replicas(&self) -> Result<u8, String> {
        self.storage
            .number_of_replicas(self.svc_name, self.api_name)
            .await
    }

    pub async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<u8, String> {
        self.storage
            .wait_for_replicas(self.svc_name, self.api_name, replicas, timeout)
            .await
    }

    pub async fn exists(&self, namespace: Option<&str>, key: &str) -> Result<bool, String> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn scan(
        &self,
        namespace: Option<&str>,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        self.storage
            .scan(
                self.svc_name,
                self.api_name,
                namespace,
                pattern,
                cursor,
                count,
            )
            .await
    }

    pub async fn length(&self, namespace: Option<&str>, key: &str) -> Result<u64, String> {
        self.storage
            .length(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn delete(&self, namespace: Option<&str>, key: &str) -> Result<(), String> {
        self.storage
            .delete(self.svc_name, self.api_name, namespace, key)
            .await
    }
}

pub struct LabelledEntityIndexedStorage<'a, S: IndexedStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    entity_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + IndexedStorage> LabelledEntityIndexedStorage<'a, S> {
    pub fn new(
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        storage: &'a S,
    ) -> Self {
        Self {
            svc_name,
            api_name,
            entity_name,
            storage,
        }
    }

    pub async fn append<V: Encode>(
        &self,
        namespace: Option<&str>,
        key: &str,
        id: u64,
        value: &V,
    ) -> Result<(), String> {
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                &serialize(value)?,
            )
            .await
    }

    pub async fn append_raw(
        &self,
        namespace: Option<&str>,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), String> {
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                value,
            )
            .await
    }

    pub async fn read<V: Decode>(
        &self,
        namespace: Option<&str>,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<V>, String> {
        self.storage
            .read(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                start_id,
                end_id,
            )
            .await
            .and_then(|values| {
                values
                    .into_iter()
                    .map(|bytes| deserialize::<V>(&bytes))
                    .collect()
            })
    }

    pub async fn read_raw(
        &self,
        namespace: Option<&str>,
        key: &str,
        from: u64,
        count: u64,
    ) -> Result<Vec<Bytes>, String> {
        self.storage
            .read(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                from,
                count,
            )
            .await
    }
}
