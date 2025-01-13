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
use golem_common::model::oplog::DurableFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::eventual::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<Option<Resource<IncomingValue>>, Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<Option<Vec<u8>>, SerializableError>::new(
            self,
            "golem keyvalue::eventual",
            "get",
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let result = self
                .state
                .key_value_service
                .get(account_id, bucket.clone(), key.clone())
                .await;
            durability.persist(self, (bucket, key), result).await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(Some(value)) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table()
                    .push(IncomingValueEntry::new(value))?;
                Ok(Ok(Some(incoming_value)))
            }
            Ok(None) => Ok(Ok(None)),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
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
        let account_id = self.owned_worker_id.account_id();
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

        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem keyvalue::eventual",
            "set",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let input = (bucket.clone(), key.clone(), outgoing_value.len() as u64);
            let result = self
                .state
                .key_value_service
                .set(account_id, bucket, key, outgoing_value)
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

    async fn delete(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem keyvalue::eventual",
            "delete",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let input = (bucket.clone(), key.clone());
            let result = self
                .state
                .key_value_service
                .delete(account_id, bucket, key)
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

    async fn exists(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        let account_id = self.owned_worker_id.account_id();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();

        let durability = Durability::<bool, SerializableError>::new(
            self,
            "golem keyvalue::eventual",
            "exists",
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let input = (bucket.clone(), key.clone());
            let result = self
                .state
                .key_value_service
                .exists(account_id, bucket, key)
                .await;
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(exists) => Ok(Ok(exists)),
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
