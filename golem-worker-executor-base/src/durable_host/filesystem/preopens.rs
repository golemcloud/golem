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

use anyhow::anyhow;
use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::DurableFunctionType;
use wasmtime_wasi::bindings::filesystem::preopens::{Descriptor, Host};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_directories(&mut self) -> anyhow::Result<Vec<(Resource<Descriptor>, String)>> {
        let durability = Durability::<Vec<String>, SerializableError>::new(
            self,
            "cli::preopens",
            "get_directories",
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let current_dirs = Host::get_directories(&mut self.as_wasi_view()).await?;

        let names = {
            if durability.is_live() {
                let result: Result<Vec<String>, anyhow::Error> = Ok(current_dirs
                    .iter()
                    .map(|(_, name)| name.clone())
                    .collect::<Vec<_>>());
                durability.persist(self, (), result).await
            } else {
                durability.replay(self).await
            }
        }?;

        // Filtering the current set of pre-opened directories by the serialized names
        let filtered = current_dirs
            .into_iter()
            .filter(|(_, name)| names.contains(name))
            .collect::<Vec<_>>();

        if filtered.len() == names.len() {
            // All directories were found
            Ok(filtered)
        } else {
            Err(anyhow!(
                "Not all previously available pre-opened directories were found"
            ))
        }
    }
}
