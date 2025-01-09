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

use crate::durable_host::serialized::{SerializableDateTime, SerializableError};
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime_wasi::bindings::clocks::wall_clock::{Datetime, Host};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Datetime> {
        let durability = Durability::<Ctx, SerializableDateTime, SerializableError>::new(
            self,
            "wall_clock",
            "now",
            WrappedFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::now(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn resolution(&mut self) -> anyhow::Result<Datetime> {
        let durability = Durability::<Ctx, SerializableDateTime, SerializableError>::new(
            self,
            "wall_clock",
            "resolution",
            WrappedFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::resolution(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }
}
