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

use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;

use golem_common::model::AccountId;

use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};

/// Service implementing a persistent key-value store
#[async_trait]
pub trait KeyValueService: Send + Sync {
    async fn delete(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()>;

    async fn delete_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<()>;

    async fn exists(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool>;

    async fn get(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>>;

    async fn get_keys(&self, account_id: AccountId, bucket: String) -> anyhow::Result<Vec<String>>;

    async fn get_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>>;

    async fn set(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()>;

    async fn set_many(
        &self,
        account_id: AccountId,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<()>;
}

#[derive(Clone, Debug)]
pub struct DefaultKeyValueService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
}

impl DefaultKeyValueService {
    pub fn new(key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>) -> Self {
        Self { key_value_storage }
    }
}

#[async_trait]
impl KeyValueService for DefaultKeyValueService {
    async fn delete(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()> {
        self.key_value_storage
            .with("key_value", "delete")
            .del(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                &key,
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn delete_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<()> {
        self.key_value_storage
            .with("key_value", "delete_many")
            .del_many(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                keys,
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn exists(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        let exists: bool = self
            .key_value_storage
            .with("key_value", "exists")
            .exists(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                &key,
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(exists)
    }

    async fn get(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let incoming_value: Option<Vec<u8>> = self
            .key_value_storage
            .with_entity("key_value", "get", "custom")
            .get_raw(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                &key,
            )
            .await
            .map_err(|err| anyhow!(err))?
            .map(|bytes| bytes.to_vec());
        Ok(incoming_value)
    }

    async fn get_keys(&self, account_id: AccountId, bucket: String) -> anyhow::Result<Vec<String>> {
        let keys: Vec<String> = self
            .key_value_storage
            .with("key_value", "get_keys")
            .keys(KeyValueStorageNamespace::UserDefined { account_id, bucket })
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(keys)
    }

    async fn get_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        let incoming_values: Vec<Option<Bytes>> = self
            .key_value_storage
            .with_entity("key_value", "get_many", "custom")
            .get_many_raw(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                keys,
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(incoming_values
            .into_iter()
            .map(|mb| mb.map(|b| b.to_vec()))
            .collect())
    }

    async fn set(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()> {
        self.key_value_storage
            .with_entity("key_value", "set", "custom")
            .set_raw(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                &key,
                &outgoing_value,
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn set_many(
        &self,
        account_id: AccountId,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        let key_values: Vec<(&str, &[u8])> = key_values
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_slice()))
            .collect();
        self.key_value_storage
            .with_entity("key_value", "set_many", "custom")
            .set_many_raw(
                KeyValueStorageNamespace::UserDefined { account_id, bucket },
                &key_values,
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }
}
