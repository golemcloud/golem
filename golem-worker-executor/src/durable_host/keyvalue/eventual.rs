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

use golem_common::model::oplog::host_functions::{
    KeyvalueEventualDelete, KeyvalueEventualExists, KeyvalueEventualGet, KeyvalueEventualSet,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestKVBucketAndKey, HostRequestKVBucketKeyAndSize,
    HostResponseKVDelete, HostResponseKVGet, HostResponseKVUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::{Durability, DurableWorkerCtx, HostFailureKind, InternalRetryResult};
use crate::metrics::storage::{
    STORAGE_TYPE_KV, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::preview2::wasi::keyvalue::eventual::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<Option<Resource<IncomingValue>>, Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let mut durability =
            Durability::<KeyvalueEventualGet>::new(self, DurableFunctionType::ReadRemote).await?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .get(environment_id, bucket.clone(), key.clone())
                    .await
                    .map_err(|err| err.to_string());
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            durability
                .persist(
                    self,
                    HostRequestKVBucketAndKey { bucket, key },
                    HostResponseKVGet { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(Some(value)) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table()
                    .push(IncomingValueEntry::new(value))?;
                Ok(Ok(Some(incoming_value)))
            }
            Ok(None) => Ok(Ok(None)),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
        outgoing_value: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let outgoing_value = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&outgoing_value)?
            .body
            .read()
            .unwrap()
            .clone();

        let mut durability =
            Durability::<KeyvalueEventualSet>::new(self, DurableFunctionType::WriteRemote).await?;

        let result = if durability.is_live() {
            let length = outgoing_value.len() as u64;
            let input = HostRequestKVBucketKeyAndSize {
                bucket: bucket.clone(),
                key: key.clone(),
                length: outgoing_value.len(),
            };
            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .set(
                        environment_id,
                        bucket.clone(),
                        key.clone(),
                        outgoing_value.clone(),
                    )
                    .await
                    .map_err(|err| err.to_string());
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_bytes_written(STORAGE_TYPE_KV, &account_id, &environment_id_str, length);
                record_storage_objects_written(STORAGE_TYPE_KV, &account_id, &environment_id_str, 1);
            }
            durability
                .persist(self, input, HostResponseKVUnit { result })
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let mut durability =
            Durability::<KeyvalueEventualDelete>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let result = if durability.is_live() {
            let input = HostRequestKVBucketAndKey {
                bucket: bucket.clone(),
                key: key.clone(),
            };
            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .delete(environment_id, bucket.clone(), key.clone())
                    .await
                    .map_err(|err| err.to_string());
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_objects_deleted(STORAGE_TYPE_KV, &account_id, &environment_id_str, 1);
            }
            durability
                .persist(self, input, HostResponseKVUnit { result })
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        let environment_id = self.owned_agent_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let mut durability =
            Durability::<KeyvalueEventualExists>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let result = if durability.is_live() {
            let input = HostRequestKVBucketAndKey {
                bucket: bucket.clone(),
                key: key.clone(),
            };
            let result = loop {
                let result = self
                    .state
                    .key_value_service
                    .exists(environment_id, bucket.clone(), key.clone())
                    .await
                    .map_err(|err| err.to_string());
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            durability
                .persist(self, input, HostResponseKVDelete { result })
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table()
                    .push(ErrorEntry::new(format!("{e:?}")))?;
                Ok(Err(error))
            }
        }
    }
}
