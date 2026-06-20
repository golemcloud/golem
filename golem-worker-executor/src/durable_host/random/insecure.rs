// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{CallHandle, NotCancellable};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestRandomBytes, HostResponseRandomBytes,
    HostResponseRandomU64, host_functions,
};
use wasmtime_wasi::p2::bindings::random::insecure::Host;
use wasmtime_wasi::random::WasiRandomView as _;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_insecure_random_bytes(&mut self, length: u64) -> wasmtime::Result<Vec<u8>> {
        let handle = CallHandle::<
            host_functions::RandomInsecureGetInsecureRandomBytes,
            NotCancellable,
        >::start(
            self,
            HostRequestRandomBytes { length },
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let bytes = {
                    let mut view = ctx.as_wasi_view();
                    Host::get_insecure_random_bytes(view.random(), length).await?
                };
                Ok(HostResponseRandomBytes { bytes })
            })
            .await?;

        Ok(result.bytes)
    }

    async fn get_insecure_random_u64(&mut self) -> wasmtime::Result<u64> {
        let handle =
            CallHandle::<host_functions::RandomInsecureGetInsecureRandomU64, NotCancellable>::start(
                self,
                HostRequestNoInput {},
                DurableFunctionType::ReadLocal,
            )
            .await?;

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let value = {
                    let mut view = ctx.as_wasi_view();
                    Host::get_insecure_random_u64(view.random()).await?
                };
                Ok(HostResponseRandomU64 { value })
            })
            .await?;

        Ok(result.value)
    }
}
