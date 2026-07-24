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

use crate::durable_host::concurrent::{
    CallHandle, NotCancellable, drain_queued_dropped_call_events,
};
use crate::durable_host::p3::DurableP3View;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    P3RandomInsecureGetInsecureRandomBytes, P3RandomInsecureGetInsecureRandomU64,
    P3RandomInsecureSeedGetInsecureSeed, P3RandomRandomGetRandomBytes, P3RandomRandomGetRandomU64,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestRandomBytes, HostResponseRandomBytes,
    HostResponseRandomSeed, HostResponseRandomU64,
};
use wasmtime_wasi::p3::bindings::random::{insecure, insecure_seed, random};
use wasmtime_wasi::random::WasiRandomView as _;

impl<Ctx: WorkerCtx> random::Host for DurableP3View<'_, Ctx> {
    async fn get_random_bytes(&mut self, len: u64) -> wasmtime::Result<Vec<u8>> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3RandomRandomGetRandomBytes, NotCancellable>::start(
            ctx,
            HostRequestRandomBytes { length: len },
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let bytes = {
                    let mut view = ctx.as_wasi_view();
                    random::Host::get_random_bytes(view.random(), len).await?
                };
                Ok(HostResponseRandomBytes { bytes })
            })
            .await?;

        Ok(result.bytes)
    }

    async fn get_random_u64(&mut self) -> wasmtime::Result<u64> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3RandomRandomGetRandomU64, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let value = {
                    let mut view = ctx.as_wasi_view();
                    random::Host::get_random_u64(view.random()).await?
                };
                Ok(HostResponseRandomU64 { value })
            })
            .await?;

        Ok(result.value)
    }
}

impl<Ctx: WorkerCtx> insecure::Host for DurableP3View<'_, Ctx> {
    async fn get_insecure_random_bytes(&mut self, len: u64) -> wasmtime::Result<Vec<u8>> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3RandomInsecureGetInsecureRandomBytes, NotCancellable>::start(
            ctx,
            HostRequestRandomBytes { length: len },
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let bytes = {
                    let mut view = ctx.as_wasi_view();
                    insecure::Host::get_insecure_random_bytes(view.random(), len).await?
                };
                Ok(HostResponseRandomBytes { bytes })
            })
            .await?;

        Ok(result.bytes)
    }

    async fn get_insecure_random_u64(&mut self) -> wasmtime::Result<u64> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3RandomInsecureGetInsecureRandomU64, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let value = {
                    let mut view = ctx.as_wasi_view();
                    insecure::Host::get_insecure_random_u64(view.random()).await?
                };
                Ok(HostResponseRandomU64 { value })
            })
            .await?;

        Ok(result.value)
    }
}

impl<Ctx: WorkerCtx> insecure_seed::Host for DurableP3View<'_, Ctx> {
    async fn get_insecure_seed(&mut self) -> wasmtime::Result<(u64, u64)> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3RandomInsecureSeedGetInsecureSeed, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let result = {
                    let mut view = ctx.as_wasi_view();
                    insecure_seed::Host::get_insecure_seed(view.random()).await?
                };
                Ok(HostResponseRandomSeed {
                    lo: result.0,
                    hi: result.1,
                })
            })
            .await?;

        Ok((result.lo, result.hi))
    }
}
