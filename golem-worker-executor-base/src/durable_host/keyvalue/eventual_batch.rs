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
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{ResourceTableError, WasiView};

use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
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
        record_host_function_call("keyvalue::eventual_batch", "get_many");
        let account_id = self.state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let result: anyhow::Result<Vec<Option<Vec<u8>>>> =
            Durability::<Ctx, Vec<Option<Vec<u8>>>, SerializableError>::wrap(
                self,
                WrappedFunctionType::ReadRemote,
                "golem keyvalue::eventual_batch::get_many",
                |ctx| {
                    ctx.state.key_value_service.get_many(
                        account_id.clone(),
                        bucket.clone(),
                        keys.clone(),
                    )
                },
            )
            .await;
        match result {
            Ok(values) => {
                let mut result = Vec::new();
                for maybe_incoming_value in values {
                    match maybe_incoming_value {
                        Some(incoming_value) => {
                            let value = self
                                .as_wasi_view()
                                .table_mut()
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
                    .table_mut()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn keys(
        &mut self,
        bucket: Resource<Bucket>,
    ) -> anyhow::Result<Result<Vec<Key>, Resource<Error>>> {
        record_host_function_call("keyvalue::eventual_batch", "get_keys");
        let account_id = self.state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let keys = Durability::<Ctx, Vec<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem keyvalue::eventual_batch::get_keys",
            |ctx| {
                ctx.state
                    .key_value_service
                    .get_keys(account_id.clone(), bucket.clone())
            },
        )
        .await?;
        Ok(Ok(keys))
    }

    async fn set_many(
        &mut self,
        bucket: Resource<Bucket>,
        key_values: Vec<(Key, Resource<OutgoingValue>)>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        record_host_function_call("keyvalue::eventual_batch", "set_many");
        let account_id = self.state.account_id.clone();
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
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem keyvalue::eventual_batch::set_many",
            |ctx| {
                ctx.state.key_value_service.set_many(
                    account_id.clone(),
                    bucket.clone(),
                    key_values.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
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
        record_host_function_call("keyvalue::eventual_batch", "delete_many");
        let account_id = self.state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem keyvalue::eventual_batch::delete_many",
            |ctx| {
                ctx.state.key_value_service.delete_many(
                    account_id.clone(),
                    bucket.clone(),
                    keys.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }
}
