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
    host_functions, DurableFunctionType, HostRequestNoInput, HostResponseRandomSeed,
};
use wasmtime_wasi::p2::bindings::random::insecure_seed::Host;
use wasmtime_wasi::random::WasiRandomView as _;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn insecure_seed(&mut self) -> wasmtime::Result<(u64, u64)> {
        let durability = Durability::<host_functions::RandomInsecureSeedInsecureSeed>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let result = if durability.is_live() {
            let result = {
                let mut view = self.as_wasi_view();
                Host::insecure_seed(view.random()).await?
            };
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseRandomSeed {
                        lo: result.0,
                        hi: result.1,
                    },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok((result.lo, result.hi))
    }
}
