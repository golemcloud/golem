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

use crate::durable_host::serialized::{SerializableDateTime, SerializableError};
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime_wasi::bindings::clocks::wall_clock::{Datetime, Host};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Datetime> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("clocks::wall_clock", "now");
        Durability::<Ctx, SerializableDateTime, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "wall_clock::now",
            |ctx| Box::pin(async { Host::now(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn resolution(&mut self) -> anyhow::Result<Datetime> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("clocks::wall_clock", "resolution");
        Durability::<Ctx, SerializableDateTime, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "wall_clock::resolution",
            |ctx| Box::pin(async { Host::resolution(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Datetime> {
        (*self).now().await
    }

    async fn resolution(&mut self) -> anyhow::Result<Datetime> {
        (*self).resolution().await
    }
}
