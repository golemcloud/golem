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

use futures::executor::block_on;
use wasmtime::component::{Accessor, HasSelf, Resource};

use crate::durable_host::concurrent::{DurableCallSession, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::p2_monotonic_clock::wasi::clocks0_2_6::monotonic_clock::{
    Duration, Host, HostWithStore, Instant, Pollable,
};
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestMonotonicClockDuration, HostRequestNoInput,
    HostResponseMonotonicClockTimestamp, host_functions,
};
use wasmtime_wasi::clocks::WasiClocksView as _;
use wasmtime_wasi::p2::bindings::clocks::monotonic_clock::Host as WasiMonotonicClockHost;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

fn current_monotonic_time<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
) -> wasmtime::Result<Instant> {
    accessor.with(|mut access| {
        let mut view = access.get().as_wasi_view();
        block_on(WasiMonotonicClockHost::now(&mut view.clocks()))
    })
}

fn current_monotonic_resolution<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
) -> wasmtime::Result<Duration> {
    accessor.with(|mut access| {
        let mut view = access.get().as_wasi_view();
        block_on(WasiMonotonicClockHost::resolution(&mut view.clocks()))
    })
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn now(accessor: &Accessor<U, Self>) -> anyhow::Result<Instant> {
        #[cfg(feature = "test-utils")]
        let (skip_durability, owner_execution) = accessor.with(|mut access| {
            let ctx = access.get();
            (
                ctx.test_should_skip_monotonic_clock_now_durability(),
                ctx.owner_execution.clone(),
            )
        });
        #[cfg(feature = "test-utils")]
        if skip_durability {
            return Ok(current_monotonic_time(accessor)?);
        }
        #[cfg(feature = "test-utils")]
        owner_execution.test_before_monotonic_clock_now().await;

        let result =
            DurableCallSession::<host_functions::MonotonicClockNow, NotCancellable>::invoke_access(
                accessor,
                accessor.getter(),
                HostRequestNoInput {},
                DurableFunctionType::ReadLocal,
                async || {
                    Ok::<_, anyhow::Error>(HostResponseMonotonicClockTimestamp {
                        nanos: current_monotonic_time(accessor)?,
                    })
                },
            )
            .await?;

        Ok(result.nanos)
    }

    async fn resolution(accessor: &Accessor<U, Self>) -> anyhow::Result<Duration> {
        let result = DurableCallSession::<
            host_functions::MonotonicClockResolution,
            NotCancellable,
        >::invoke_access(
            accessor,
            accessor.getter(),
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async || {
                Ok::<_, anyhow::Error>(HostResponseMonotonicClockTimestamp {
                    nanos: current_monotonic_resolution(accessor)?,
                })
            },
        )
        .await?;

        Ok(result.nanos)
    }

    async fn subscribe_instant(
        accessor: &Accessor<U, Self>,
        when: Instant,
    ) -> anyhow::Result<Resource<Pollable>> {
        Ok(accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("monotonic_clock", "subscribe_instant");
            let mut view = ctx.as_wasi_view();
            block_on(WasiMonotonicClockHost::subscribe_instant(
                &mut view.clocks(),
                when,
            ))
        })?)
    }

    async fn subscribe_duration(
        accessor: &Accessor<U, Self>,
        duration_in_nanos: Duration,
    ) -> anyhow::Result<Resource<Pollable>> {
        let now = DurableCallSession::<
            host_functions::MonotonicClockSubscribeDuration,
            NotCancellable,
        >::invoke_access(
            accessor,
            accessor.getter(),
            HostRequestMonotonicClockDuration { duration_in_nanos },
            DurableFunctionType::ReadLocal,
            async || {
                Ok::<_, anyhow::Error>(HostResponseMonotonicClockTimestamp {
                    nanos: current_monotonic_time(accessor)?,
                })
            },
        )
        .await?;

        let worker = accessor.with(|mut access| access.get().public_state.worker().clone());
        worker
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
        let when = now.nanos.saturating_add(duration_in_nanos);
        Ok(accessor.with(|mut access| {
            let mut view = access.get().as_wasi_view();
            block_on(WasiMonotonicClockHost::subscribe_instant(
                &mut view.clocks(),
                when,
            ))
        })?)
    }
}
