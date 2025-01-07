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

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime_wasi::bindings::random::insecure::Host;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_insecure_random_bytes(&mut self, len: u64) -> anyhow::Result<Vec<u8>> {
        record_host_function_call("random::insecure", "get_insecure_random_bytes");
        Durability::<Ctx, (), Vec<u8>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem random::insecure::get_insecure_random_bytes",
            (),
            |ctx| {
                Box::pin(async move {
                    Host::get_insecure_random_bytes(&mut ctx.as_wasi_view(), len).await
                })
            },
        )
        .await
    }

    async fn get_insecure_random_u64(&mut self) -> anyhow::Result<u64> {
        record_host_function_call("random::insecure", "get_insecure_random_u64");
        Durability::<Ctx, (), u64, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem random::insecure::get_insecure_random_u64",
            (),
            |ctx| Box::pin(async { Host::get_insecure_random_u64(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}
