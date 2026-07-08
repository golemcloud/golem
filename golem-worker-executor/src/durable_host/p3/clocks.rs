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
use crate::durable_host::p3::{DurableP3, DurableP3View, run_read_access};
use crate::durable_host::suspendable_wait::{
    ParkOutcome, SuspendableWaitContext, ephemeral_sleep_too_long_error, park_suspendable_wait,
};
use crate::workerctx::WorkerCtx;
use chrono::Utc;
use futures::executor::block_on;
use golem_common::model::Timestamp;
use golem_common::model::oplog::host_functions::{
    P3MonotonicClockGetResolution, P3MonotonicClockNow, P3MonotonicClockWaitFor,
    P3MonotonicClockWaitUntil, P3SystemClockGetResolution, P3SystemClockNow,
};
use golem_common::model::oplog::types::SerializableDateTime;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestMonotonicClockDuration, HostRequestMonotonicClockTimestamp,
    HostRequestNoInput, HostResponseMonotonicClockTimestamp, HostResponseP3MonotonicClockUnit,
    HostResponseWallClock,
};
use golem_service_base::error::worker_executor::InterruptKind;
use std::time::Duration;
use wasmtime::component::Accessor;
use wasmtime_wasi::clocks::WasiClocksView;
use wasmtime_wasi::p3::bindings::clocks::{monotonic_clock, system_clock, types};

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> system_clock::Host for DurableP3View<'_, Ctx> {
    async fn now(&mut self) -> wasmtime::Result<system_clock::Instant> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3SystemClockNow, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let result = {
                    let mut view = ctx.as_wasi_view();
                    system_clock::Host::now(&mut view.clocks()).await?
                };
                Ok(HostResponseWallClock {
                    time: SerializableDateTime::from(result),
                })
            })
            .await?;

        Ok(result.time.into())
    }

    async fn get_resolution(&mut self) -> wasmtime::Result<types::Duration> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3SystemClockGetResolution, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    system_clock::Host::get_resolution(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            })
            .await?;

        Ok(result.nanos)
    }
}

impl<Ctx: WorkerCtx> monotonic_clock::Host for DurableP3View<'_, Ctx> {
    async fn now(&mut self) -> wasmtime::Result<monotonic_clock::Mark> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3MonotonicClockNow, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    monotonic_clock::Host::now(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            })
            .await?;

        Ok(result.nanos)
    }

    async fn get_resolution(&mut self) -> wasmtime::Result<types::Duration> {
        let ctx = self.0.durable_ctx_mut();
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(wasmtime::Error::from)?;
        let handle = CallHandle::<P3MonotonicClockGetResolution, NotCancellable>::start(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(ctx, async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    monotonic_clock::Host::get_resolution(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            })
            .await?;

        Ok(result.nanos)
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> monotonic_clock::HostWithStore<U> for DurableP3<Ctx> {
    async fn wait_until(
        store: &Accessor<U, Self>,
        when: monotonic_clock::Mark,
    ) -> wasmtime::Result<()> {
        run_read_access::<_, _, Ctx, P3MonotonicClockWaitUntil, _, _>(
            store,
            HostRequestMonotonicClockTimestamp { nanos: when },
            DurableFunctionType::ReadLocal,
            || async {
                wait_until_live::<U, Ctx>(store, when).await?;
                Ok(HostResponseP3MonotonicClockUnit {})
            },
        )
        .await
        .map(|_| ())
    }

    async fn wait_for(
        store: &Accessor<U, Self>,
        how_long: types::Duration,
    ) -> wasmtime::Result<()> {
        let recorded_now = run_read_access::<_, _, Ctx, P3MonotonicClockNow, _, _>(
            store,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            || async {
                let nanos = current_monotonic_now::<U, Ctx>(store)?;
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            },
        )
        .await?
        .nanos;
        let when = recorded_now.saturating_add(how_long);

        run_read_access::<_, _, Ctx, P3MonotonicClockWaitFor, _, _>(
            store,
            HostRequestMonotonicClockDuration {
                duration_in_nanos: how_long,
            },
            DurableFunctionType::ReadLocal,
            || async {
                wait_until_live::<U, Ctx>(store, when).await?;
                Ok(HostResponseP3MonotonicClockUnit {})
            },
        )
        .await
        .map(|_| ())
    }
}

async fn wait_until_live<U: Send + 'static, Ctx: WorkerCtx>(
    store: &Accessor<U, DurableP3<Ctx>>,
    when: monotonic_clock::Mark,
) -> wasmtime::Result<()> {
    let context = store.with(|mut access| {
        let ctx = super::expect_ctx::<Ctx, U>(access.data_mut()).durable_ctx_mut();
        let now = block_on(monotonic_clock::Host::now(&mut ctx.as_wasi_view().clocks()))?;
        let remaining = remaining_duration(now, when);
        Ok::<_, wasmtime::Error>(SuspendableWaitContext {
            wait_id: ctx.state.next_suspendable_wait_id(),
            agent_mode: ctx.agent_mode(),
            suspend: ctx.state.config.suspend.clone(),
            wait_deadline: Some(Utc::now() + chrono::Duration::from_std(remaining).unwrap()),
            suspendable_waits: ctx.state.suspendable_waits(),
            wakeup_scheduler: ctx.state.wakeup_scheduler(),
        })
    })?;

    let outcome = park_suspendable_wait(
        context,
        || async move {
            if let Ok(now) = current_monotonic_now::<U, Ctx>(store) {
                tokio::time::sleep(remaining_duration(now, when)).await;
            }
        },
        || {
            current_monotonic_now::<U, Ctx>(store)
                .map(|now| now >= when)
                .unwrap_or(false)
        },
        || {
            store.with(|mut access| {
                let ctx = super::expect_ctx::<Ctx, U>(access.data_mut()).durable_ctx_mut();
                ctx.state.safe_to_suspend()
            })
        },
        || {
            current_monotonic_now::<U, Ctx>(store)
                .ok()
                .map(|now| remaining_duration(now, when))
        },
    )
    .await?;

    match outcome {
        ParkOutcome::Ready => Ok(()),
        ParkOutcome::SuspendWorker => Err(wasmtime::Error::from_anyhow(
            InterruptKind::Suspend(Timestamp::now_utc()).into(),
        )),
        ParkOutcome::EphemeralTooLong {
            requested_nanos,
            max_nanos,
        } => Err(ephemeral_sleep_too_long_error(requested_nanos, max_nanos)),
    }
}

fn current_monotonic_now<U: Send + 'static, Ctx: WorkerCtx>(
    store: &Accessor<U, DurableP3<Ctx>>,
) -> wasmtime::Result<monotonic_clock::Mark> {
    store.with(|mut access| {
        let ctx = super::expect_ctx::<Ctx, U>(access.data_mut()).durable_ctx_mut();
        block_on(monotonic_clock::Host::now(&mut ctx.as_wasi_view().clocks()))
    })
}

fn remaining_duration(now: monotonic_clock::Mark, when: monotonic_clock::Mark) -> Duration {
    Duration::from_nanos(when.saturating_sub(now))
}
