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

use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{
    host_functions, DurableFunctionType, HostRequestNoInput, HostRequestRandomBytes,
    HostResponseRandomBytes, HostResponseRandomU64,
};
use wasmtime_wasi::p2::bindings::random::random::Host;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_random_bytes(&mut self, length: u64) -> anyhow::Result<Vec<u8>> {
        let durability = Durability::<host_functions::RandomGetRandomBytes>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let result = if durability.is_live() {
            let bytes = Host::get_random_bytes(&mut self.as_wasi_view(), length).await?; // this supposed to never fail
            durability
                .persist(
                    self,
                    HostRequestRandomBytes { length },
                    HostResponseRandomBytes { bytes },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.bytes)
    }

    async fn get_random_u64(&mut self) -> anyhow::Result<u64> {
        let durability = Durability::<host_functions::RandomGetRandomU64>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let result = if durability.is_live() {
            let value = Host::get_random_u64(&mut self.as_wasi_view()).await?; // this supposed to never fail
            durability
                .persist(self, HostRequestNoInput {}, HostResponseRandomU64 { value })
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.value)
    }
}
