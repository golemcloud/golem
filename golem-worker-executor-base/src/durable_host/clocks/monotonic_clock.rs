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
use wasmtime::component::Resource;

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime_wasi::bindings::clocks::monotonic_clock::{Duration, Host, Instant, Pollable};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Instant> {
        record_host_function_call("clocks::monotonic_clock", "now");
        Durability::<Ctx, (), Instant, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "monotonic_clock::now",
            (),
            |ctx| Box::pin(async { Host::now(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn resolution(&mut self) -> anyhow::Result<Instant> {
        record_host_function_call("clocks::monotonic_clock", "resolution");
        Durability::<Ctx, (), Instant, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "monotonic_clock::resolution",
            (),
            |ctx| Box::pin(async { Host::resolution(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn subscribe_instant(&mut self, when: Instant) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("clocks::monotonic_clock", "subscribe_instant");
        Host::subscribe_instant(&mut self.as_wasi_view(), when).await
    }

    async fn subscribe_duration(&mut self, when: Duration) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("clocks::monotonic_clock", "subscribe_duration");
        let now = Durability::<Ctx, (), Instant, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "monotonic_clock::now", // should be 'subscribe_duration' but have to keep for backward compatibility with Golem 1.0
            (),
            |ctx| Box::pin(async { Host::now(&mut ctx.as_wasi_view()).await }),
        )
        .await?;
        self.state.oplog.commit(CommitLevel::DurableOnly).await;
        let when = now.saturating_add(when);
        Host::subscribe_instant(&mut self.as_wasi_view(), when).await
    }
}
