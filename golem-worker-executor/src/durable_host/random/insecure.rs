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

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::DurableFunctionType;
use wasmtime_wasi::bindings::random::insecure::Host;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_insecure_random_bytes(&mut self, len: u64) -> anyhow::Result<Vec<u8>> {
        let durability = Durability::<Vec<u8>, SerializableError>::new(
            self,
            "golem random::insecure",
            "get_insecure_random_bytes",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::get_insecure_random_bytes(&mut self.as_wasi_view(), len).await;
            durability.persist(self, len, result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn get_insecure_random_u64(&mut self) -> anyhow::Result<u64> {
        let durability = Durability::<u64, SerializableError>::new(
            self,
            "golem random::insecure",
            "get_insecure_random_u64",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::get_insecure_random_u64(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }
}
