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
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::DurableFunctionType;
use wasmtime_wasi::bindings::cli::environment::Host;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        let durability = Durability::<Vec<(String, String)>, SerializableError>::new(
            self,
            "golem_environment",
            "get_environment",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::get_environment(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn get_arguments(&mut self) -> anyhow::Result<Vec<String>> {
        let durability = Durability::<Vec<String>, SerializableError>::new(
            self,
            "golem_environment",
            "get_arguments",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::get_arguments(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn initial_cwd(&mut self) -> anyhow::Result<Option<String>> {
        let durability = Durability::<Option<String>, SerializableError>::new(
            self,
            "golem_environment",
            "get_arguments", // TODO: fix in 2.0 - for backward compatibility with Golem 1.0
            DurableFunctionType::ReadLocal,
        )
        .await?;

        if durability.is_live() {
            let result = Host::initial_cwd(&mut self.as_wasi_view()).await;
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }
    }
}
