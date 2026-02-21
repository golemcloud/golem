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

use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::eventual_batch::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    KeyvalueEventualBatchDeleteMany, KeyvalueEventualBatchGetKeys, KeyvalueEventualBatchGetMany,
    KeyvalueEventualBatchSetMany,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestKVBucket, HostRequestKVBucketAndKeySizePairs,
    HostRequestKVBucketAndKeys, HostResponseKVGetMany, HostResponseKVKeys, HostResponseKVUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::{IoView, ResourceTableError};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Vec<Key>,
    ) -> anyhow::Result<Result<Vec<Option<Resource<IncomingValue>>>, Resource<Error>>> {
        let environment_id = self.owned_worker_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability =
            Durability::<KeyvalueEventualBatchGetMany>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
            let input = HostRequestKVBucketAndKeys {
                bucket: bucket.clone(),
                keys: keys.clone(),
            };
            let result = self
                .state
                .key_value_service
                .get_many(environment_id, bucket, keys)
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(self, input, HostResponseKVGetMany { result })
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(values) => {
                let mut result = Vec::new();
                for maybe_incoming_value in values {
                    match maybe_incoming_value {
                        Some(incoming_value) => {
                            let value = self
                                .as_wasi_view()
                                .table()
                                .push(IncomingValueEntry::new(incoming_value))?;
                            result.push(Some(value));
                        }
                        None => {
                            result.push(None);
                        }
                    }
                }
                Ok(Ok(result))
            }
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn keys(
        &mut self,
        bucket: Resource<Bucket>,
    ) -> anyhow::Result<Result<Vec<Key>, Resource<Error>>> {
        let environment_id = self.owned_worker_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability =
            Durability::<KeyvalueEventualBatchGetKeys>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
            let result = self
                .state
                .key_value_service
                .get_keys(environment_id, bucket.clone())
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestKVBucket { bucket },
                    HostResponseKVKeys { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(keys) => Ok(Ok(keys)),
            Err(e) => {
                let error = self.as_wasi_view().table().push(ErrorEntry::new(e))?;
                Ok(Err(error))
            }
        }
    }

    async fn set_many(
        &mut self,
        bucket: Resource<Bucket>,
        key_values: Vec<(Key, Resource<OutgoingValue>)>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let environment_id = self.owned_worker_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let key_values = key_values
            .into_iter()
            .map(|(key, outgoing_value)| {
                let outgoing_value = self
                    .as_wasi_view()
                    .table()
                    .get::<OutgoingValueEntry>(&outgoing_value)?
                    .body
                    .read()
                    .unwrap()
                    .clone();
                Ok((key, outgoing_value))
            })
            .collect::<Result<Vec<(String, Vec<u8>)>, ResourceTableError>>()?;

        let durability =
            Durability::<KeyvalueEventualBatchSetMany>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let result = if durability.is_live() {
            let input = HostRequestKVBucketAndKeySizePairs {
                bucket: bucket.clone(),
                keys: key_values
                    .iter()
                    .map(|(k, v)| (k.clone(), v.len()))
                    .collect(),
            };
            let result = self
                .state
                .key_value_service
                .set_many(environment_id, bucket, key_values)
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
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

    async fn delete_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Vec<Key>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let project_id = self.owned_worker_id.environment_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<KeyvalueEventualBatchDeleteMany>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let input = HostRequestKVBucketAndKeys {
                bucket: bucket.clone(),
                keys: keys.clone(),
            };
            let result = self
                .state
                .key_value_service
                .delete_many(project_id, bucket, keys)
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
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
}
