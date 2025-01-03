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
use wasmtime_wasi::bindings::cli::environment::Host;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        record_host_function_call("cli::environment", "get_environment");
        Durability::<Ctx, (), Vec<(String, String)>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem_environment::get_environment",
            (),
            |ctx| Box::pin(async { Host::get_environment(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn get_arguments(&mut self) -> anyhow::Result<Vec<String>> {
        record_host_function_call("cli::environment", "get_arguments");
        Durability::<Ctx, (), Vec<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem_environment::get_arguments",
            (),
            |ctx| Box::pin(async { Host::get_arguments(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn initial_cwd(&mut self) -> anyhow::Result<Option<String>> {
        record_host_function_call("cli::environment", "initial_cwd");
        Durability::<Ctx, (), Option<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem_environment::get_arguments", // NOTE: for backward compatibility with Golem 1.0
            (),
            |ctx| Box::pin(async { Host::initial_cwd(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}
