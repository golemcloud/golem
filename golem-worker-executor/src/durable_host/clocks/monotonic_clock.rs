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

use wasmtime::component::Resource;

use crate::durable_host::concurrent::{CallHandle, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestMonotonicClockDuration, HostRequestNoInput,
    HostResponseMonotonicClockTimestamp, host_functions,
};
use wasmtime_wasi::clocks::WasiClocksView as _;
use wasmtime_wasi::p2::bindings::clocks::monotonic_clock::{Duration, Host, Instant, Pollable};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> wasmtime::Result<Instant> {
        // `now()` is a re-executable `ReadLocal`, so it uses the `CallHandle::run` combinator: the
        // live clock read is supplied as the action and is run on the live path or re-run if replay
        // finds the `Start` without its `End`; a committed `End` replays without touching the clock.
        let handle = CallHandle::<host_functions::MonotonicClockNow, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let mut view = ctx.as_wasi_view();
                let nanos = Host::now(&mut view.clocks()).await?;
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            })
            .await?;

        Ok(result.nanos)
    }

    async fn resolution(&mut self) -> wasmtime::Result<Instant> {
        let handle = CallHandle::<host_functions::MonotonicClockResolution, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    Host::resolution(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            })
            .await?;

        Ok(result.nanos)
    }

    async fn subscribe_instant(&mut self, when: Instant) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("monotonic_clock", "subscribe_instant");
        let mut view = self.as_wasi_view();
        Host::subscribe_instant(&mut view.clocks(), when).await
    }

    async fn subscribe_duration(
        &mut self,
        duration_in_nanos: Duration,
    ) -> wasmtime::Result<Resource<Pollable>> {
        let handle =
            CallHandle::<host_functions::MonotonicClockSubscribeDuration, NotCancellable>::start(
                self,
                HostRequestMonotonicClockDuration { duration_in_nanos },
                DurableFunctionType::ReadLocal,
            )
            .await?;

        let now = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    Host::now(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            })
            .await?;

        self.public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
        let when = now.nanos.saturating_add(duration_in_nanos);
        let mut view = self.as_wasi_view();
        Host::subscribe_instant(&mut view.clocks(), when).await
    }
}
