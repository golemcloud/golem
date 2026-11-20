// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! The key-value service of a never-live owner. It refuses every write and lets the reads
//! through.

use super::*;

pub(crate) struct RefusingKeyValueService {
    pub(crate) inner: Arc<dyn KeyValueService>,
}

#[async_trait]
impl KeyValueService for RefusingKeyValueService {
    async fn delete(
        &self,
        _environment_id: EnvironmentId,
        _bucket: String,
        _key: String,
    ) -> anyhow::Result<()> {
        Err(crate::durable_host::never_live::refusal("keyvalue::delete").into())
    }

    async fn delete_many(
        &self,
        _environment_id: EnvironmentId,
        _bucket: String,
        _keys: Arc<[String]>,
    ) -> anyhow::Result<()> {
        Err(crate::durable_host::never_live::refusal("keyvalue::delete_many").into())
    }

    async fn exists(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        self.inner.exists(environment_id, bucket, key).await
    }

    async fn get(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner.get(environment_id, bucket, key).await
    }

    async fn get_keys(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
    ) -> anyhow::Result<Vec<String>> {
        self.inner.get_keys(environment_id, bucket).await
    }

    async fn get_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        keys: Arc<[String]>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        self.inner.get_many(environment_id, bucket, keys).await
    }

    async fn set(
        &self,
        _environment_id: EnvironmentId,
        _bucket: String,
        _key: String,
        _outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()> {
        Err(crate::durable_host::never_live::refusal("keyvalue::set").into())
    }

    async fn set_many(
        &self,
        _environment_id: EnvironmentId,
        _bucket: String,
        _key_values: Arc<[(String, Vec<u8>)]>,
    ) -> anyhow::Result<()> {
        Err(crate::durable_host::never_live::refusal("keyvalue::set_many").into())
    }
}
