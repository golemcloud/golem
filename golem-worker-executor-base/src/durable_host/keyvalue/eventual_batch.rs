// Copyright 2024-2025 Golem Cloud
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
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::{ResourceTableError, WasiView};

use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::eventual_batch::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Vec<Key>,
    ) -> anyhow::Result<Result<Vec<Option<Resource<IncomingValue>>>, Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<Ctx, Vec<Option<Vec<u8>>>, SerializableError>::new(
            self,
            "golem keyvalue::eventual_batch",
            "get_many",
            WrappedFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let input = (bucket.clone(), keys.clone());
            let result = self
                .state
                .key_value_service
                .get_many(account_id, bucket, keys)
                .await;
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        match result {
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
                let error = self
                    .as_wasi_view()
                    .table()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn keys(
        &mut self,
        bucket: Resource<Bucket>,
    ) -> anyhow::Result<Result<Vec<Key>, Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<Ctx, Vec<String>, SerializableError>::new(
            self,
            "golem keyvalue::eventual_batch",
            "get_keys",
            WrappedFunctionType::ReadRemote,
        )
        .await?;
        let keys = if durability.is_live() {
            let result = self
                .state
                .key_value_service
                .get_keys(account_id, bucket.clone())
                .await;
            durability.persist(self, bucket, result).await
        } else {
            durability.replay(self).await
        }?;

        Ok(Ok(keys))
    }

    async fn set_many(
        &mut self,
        bucket: Resource<Bucket>,
        key_values: Vec<(Key, Resource<OutgoingValue>)>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
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

        let durability = Durability::<Ctx, (), SerializableError>::new(
            self,
            "golem keyvalue::eventual_batch",
            "set_many",
            WrappedFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let input: (String, Vec<(String, u64)>) = (
                bucket.clone(),
                key_values
                    .iter()
                    .map(|(k, v)| (k.clone(), v.len() as u64))
                    .collect(),
            );
            let result = self
                .state
                .key_value_service
                .set_many(account_id, bucket, key_values)
                .await;
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn delete_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Vec<Key>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<Ctx, (), SerializableError>::new(
            self,
            "golem keyvalue::eventual_batch",
            "delete_many",
            WrappedFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let input = (bucket.clone(), keys.clone());
            let result = self
                .state
                .key_value_service
                .delete_many(account_id, bucket, keys)
                .await;
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }
}
