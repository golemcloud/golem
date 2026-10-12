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

use crate::durable_host::DurabilityHost;
use crate::durable_host::concurrent::{
    AccessClaimOptions, Cancellable, DeferredCallReplayOutcome, DurableCallSession,
    HostInternalCallOwner, HostInternalReplay, NotCancellable,
};
use crate::durable_host::p3::{DurableP3, DurableP3View, run_read_access};
use crate::durable_host::replay_state::RecordedChildClaim;
use crate::durable_host::suspendable_wait::{
    ParkOutcome, ephemeral_sleep_too_long_error, wait_for_ready,
};
use crate::workerctx::WorkerCtx;
use futures::executor::block_on;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::host_functions::{
    P3MonotonicClockGetResolution, P3MonotonicClockNow, P3MonotonicClockWaitFor,
    P3MonotonicClockWaitUntil, P3SystemClockGetResolution, P3SystemClockNow,
};
use golem_common::model::oplog::types::SerializableDateTime;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestMonotonicClockDuration,
    HostRequestMonotonicClockTimestamp, HostRequestNoInput, HostResponseMonotonicClockTimestamp,
    HostResponseP3MonotonicClockUnit, HostResponseWallClock, OplogIndex,
};
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::Accessor;
use wasmtime_wasi::clocks::WasiClocksView;
use wasmtime_wasi::p3::bindings::clocks::{monotonic_clock, system_clock, types};

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> system_clock::Host for DurableP3View<'_, Ctx> {
    async fn now(&mut self) -> wasmtime::Result<system_clock::Instant> {
        let ctx = self.0.durable_ctx_mut();
        let result = DurableCallSession::<P3SystemClockNow, NotCancellable>::invoke(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| -> wasmtime::Result<_> {
                let result = {
                    let mut view = ctx.as_wasi_view();
                    system_clock::Host::now(&mut view.clocks()).await?
                };
                Ok(HostResponseWallClock {
                    time: SerializableDateTime::from(result),
                })
            },
        )
        .await?;

        Ok(result.time.into())
    }

    async fn get_resolution(&mut self) -> wasmtime::Result<types::Duration> {
        let ctx = self.0.durable_ctx_mut();
        let result = DurableCallSession::<P3SystemClockGetResolution, NotCancellable>::invoke(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    system_clock::Host::get_resolution(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            },
        )
        .await?;

        Ok(result.nanos)
    }
}

impl<Ctx: WorkerCtx> monotonic_clock::Host for DurableP3View<'_, Ctx> {
    async fn now(&mut self) -> wasmtime::Result<monotonic_clock::Mark> {
        let ctx = self.0.durable_ctx_mut();
        let result = DurableCallSession::<P3MonotonicClockNow, NotCancellable>::invoke(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    monotonic_clock::Host::now(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            },
        )
        .await?;

        Ok(result.nanos)
    }

    async fn get_resolution(&mut self) -> wasmtime::Result<types::Duration> {
        let ctx = self.0.durable_ctx_mut();
        let result = DurableCallSession::<P3MonotonicClockGetResolution, NotCancellable>::invoke(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| -> wasmtime::Result<_> {
                let nanos = {
                    let mut view = ctx.as_wasi_view();
                    monotonic_clock::Host::get_resolution(&mut view.clocks()).await?
                };
                Ok(HostResponseMonotonicClockTimestamp { nanos })
            },
        )
        .await?;

        Ok(result.nanos)
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> monotonic_clock::HostWithStore<U> for DurableP3<Ctx> {
    async fn wait_until(
        store: &Accessor<U, Self>,
        when: monotonic_clock::Mark,
    ) -> wasmtime::Result<()> {
        // A timer must return STARTED even when its deadline has passed. An inline live
        // delivery followed by a direct host call can deadlock replay when that call holds
        // the Store before the replayed timer reaches its guest-delivery boundary.
        tokio::task::yield_now().await;
        run_read_access::<_, _, Ctx, P3MonotonicClockWaitUntil, _>(
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
        let request = HostRequestMonotonicClockDuration {
            duration_in_nanos: how_long,
        };
        // A guest can have several waits pending at once. Waits of one length record the same
        // `Start`, so they start in turn, in the order in which the guest made them. Nothing
        // may be awaited before this: the turn is taken while the guest makes the call.
        let (start_order, never_live) = store.with(|mut access| {
            let ctx = super::expect_ctx::<Ctx, U>(access.data_mut()).durable_ctx_mut();
            (
                ctx.state.replay_state.start_order(
                    P3MonotonicClockWaitFor::FQFN,
                    ctx.child_parent_start_index(
                        &DurableFunctionType::ReadLocal,
                        OplogIndex::INITIAL,
                    ),
                    how_long,
                ),
                ctx.rejects_live_continuation_at_replay_tail(),
            )
        });
        let start_order = Arc::new(start_order.lock_owned().await);
        // The first run reads the clock right after it starts the wait. When that read is in the
        // record, replay makes it before it resolves the wait: a wait that the guest dropped has
        // no terminal, and waiting for one would leave the recorded read unclaimed. The claim
        // of the wait therefore takes the read's `Start` with it. It does so when the read has
        // a recorded terminal, because replaying it then only reads the record, and when the
        // read has none in a Store that never goes live, where it can only wait with the wait.
        let recorded_read = RecordedChildClaim {
            function_name: P3MonotonicClockNow::HOST_FUNCTION_NAME,
            function_type: DurableFunctionType::ReadLocal,
            claim_open: never_live,
        };
        // The outer session owns cancellation throughout deadline reconstruction, including
        // after the internal read completed but before the timer started waiting.
        let (mut wait, recorded_read) =
            DurableCallSession::<P3MonotonicClockWaitFor, Cancellable>::start_access_with_recorded_child(
                store,
                super::durable_worker_ctx::<Ctx, U>,
                DurableFunctionType::ReadLocal,
                AccessClaimOptions {
                    // On replay each wait claims the `Start` that recorded its own length.
                    request_identity: Some(request.clone().into()),
                    start_order: Some(start_order),
                    ..Default::default()
                },
                Some(recorded_read),
                async move |_| Ok(request),
            )
            .await?;
        let owner = wait.host_internal_owner();
        let live = wait.is_live();
        let (owner, recorded_now) = match recorded_read {
            Some(read) => {
                let replay = HostInternalReplay::Claimed(read);
                match read_clock_for_wait::<U, Ctx>(store, owner, replay).await {
                    Ok(now) => (None, Some(now)),
                    Err(error) => {
                        wait.abandon_for_trap();
                        return Err(error);
                    }
                }
            }
            None => (Some(owner), None),
        };
        let (mut wait, replayed) = if live {
            (Some(wait), None)
        } else {
            // Without a claimed read the outer call is resolved before the read is made. That
            // is a record without a read, where the wait may have been cancelled before it, and
            // a read without a terminal in a Store that can go live, where an incomplete replay
            // must own a live cancellation guard before a child can park.
            match wait
                .replay_access_deferred(store, super::durable_worker_ctx::<Ctx, U>)
                .await?
            {
                DeferredCallReplayOutcome::Replayed(response, delivery) => {
                    (None, Some((response, delivery)))
                }
                DeferredCallReplayOutcome::Incomplete(wait) => (Some(wait), None),
            }
        };
        let now = match recorded_now {
            Some(now) => now,
            None => {
                let owner = owner.expect("a wait reads the clock once");
                let replay = if replayed.is_some() {
                    HostInternalReplay::Completed
                } else {
                    HostInternalReplay::Any
                };
                match read_clock_for_wait::<U, Ctx>(store, owner, replay).await {
                    Ok(now) => now,
                    Err(error) => {
                        if let Some(wait) = &mut wait {
                            wait.abandon_for_trap();
                        }
                        if let Some((_, delivery)) = replayed {
                            delivery.suppress();
                        }
                        return Err(error);
                    }
                }
            }
        };
        let when = now.nanos.saturating_add(how_long);

        // Yield after reconstructing the deadline, not before reading the clock: the timer
        // keeps its original deadline while live and replay both use deferred guest delivery.
        tokio::task::yield_now().await;
        if let Some((_, delivery)) = replayed {
            if delivery.is_replay_discarded() {
                std::future::pending::<()>().await;
            }
            delivery.deliver_at_accessor_terminal(store).await?;
            return Ok(());
        }
        wait.expect("an incomplete wait retains its session")
            .run_live_action_access(store, super::durable_worker_ctx::<Ctx, U>, async || {
                wait_until_live::<U, Ctx>(store, when).await?;
                Ok::<_, wasmtime::Error>(HostResponseP3MonotonicClockUnit {})
            })
            .await
            .map(|_| ())
    }
}

/// The clock read that a `wait-for` makes under its own `Start`.
async fn read_clock_for_wait<U: Send + 'static, Ctx: WorkerCtx>(
    store: &Accessor<U, DurableP3<Ctx>>,
    wait: HostInternalCallOwner,
    replay: HostInternalReplay,
) -> wasmtime::Result<HostResponseMonotonicClockTimestamp> {
    DurableCallSession::<P3MonotonicClockNow, Cancellable>::invoke_host_internal_access(
        store,
        super::durable_worker_ctx::<Ctx, U>,
        wait,
        replay,
        HostRequestNoInput {},
        DurableFunctionType::ReadLocal,
        async || {
            let nanos = current_monotonic_now::<U, Ctx>(store)?;
            Ok::<_, wasmtime::Error>(HostResponseMonotonicClockTimestamp { nanos })
        },
    )
    .await
}

async fn wait_until_live<U: Send + 'static, Ctx: WorkerCtx>(
    store: &Accessor<U, DurableP3<Ctx>>,
    when: monotonic_clock::Mark,
) -> wasmtime::Result<()> {
    let runtime = store.with(|mut access| {
        let ctx = super::expect_ctx::<Ctx, U>(access.data_mut()).durable_ctx_mut();
        (ctx.agent_mode() == AgentMode::Durable && !ctx.state.durability_is_suppressed())
            .then(|| ctx.runtime_suspension.clone())
            .flatten()
    });
    if let Some(runtime) = runtime {
        let remaining = remaining_duration(current_monotonic_now::<U, Ctx>(store)?, when);
        let deadline = std::time::Instant::now() + remaining;
        let activity = store
            .runtime_activity()
            .and_then(|id| runtime.timer(id, deadline))
            .ok_or_else(|| wasmtime::Error::msg("timer runtime activity is not registered"))?;
        let interrupt = store.with(|mut access| {
            super::expect_ctx::<Ctx, U>(access.data_mut())
                .durable_ctx_mut()
                .create_interrupt_signal()
        });
        return activity
            .wait(tokio::time::sleep_until(deadline.into()), interrupt)
            .await
            .map_err(|kind| wasmtime::Error::from_anyhow(kind.into()));
    }
    let (mode, max_sleep, remaining, interrupt) = store.with(|mut access| {
        let ctx = super::expect_ctx::<Ctx, U>(access.data_mut()).durable_ctx_mut();
        let now = block_on(monotonic_clock::Host::now(&mut ctx.as_wasi_view().clocks()))?;
        let remaining = remaining_duration(now, when);
        let interrupt = ctx.create_interrupt_signal();
        Ok::<_, wasmtime::Error>((
            ctx.agent_mode(),
            ctx.state.config.suspend.ephemeral_max_sleep,
            remaining,
            interrupt,
        ))
    })?;

    let outcome = wait_for_ready(
        mode,
        max_sleep,
        Some(remaining),
        interrupt,
        tokio::time::sleep(remaining),
    )
    .await;

    match outcome {
        ParkOutcome::Ready => Ok(()),
        ParkOutcome::Interrupted(kind) => Err(wasmtime::Error::from_anyhow(kind.into())),
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
