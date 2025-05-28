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

use wasmtime::component::Resource;

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::DurableFunctionType;
use wasmtime_wasi::bindings::clocks::monotonic_clock::{Duration, Host, Instant, Pollable};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Instant> {
        let durability = Durability::<Instant, SerializableError>::new(
            self,
            "monotonic_clock",
            "now",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::now(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn resolution(&mut self) -> anyhow::Result<Instant> {
        let durability = Durability::<Instant, SerializableError>::new(
            self,
            "monotonic_clock",
            "resolution",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::resolution(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn subscribe_instant(&mut self, when: Instant) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call("clocks::monotonic_clock", "subscribe_instant");
        Host::subscribe_instant(&mut self.as_wasi_view(), when).await
    }

    async fn subscribe_duration(&mut self, when: Duration) -> anyhow::Result<Resource<Pollable>> {
        let durability = Durability::<Instant, SerializableError>::new(
            self,
            "monotonic_clock",
            "subscribe_duration",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let now = {
            if durability.is_live() {
                let result = Host::now(&mut self.as_wasi_view()).await;
                durability.persist(self, (), result).await
            } else {
                durability.replay(self).await
            }
        }?;

        self.state.oplog.commit(CommitLevel::DurableOnly).await;
        let when = now.saturating_add(when);
        Host::subscribe_instant(&mut self.as_wasi_view(), when).await
    }
}
