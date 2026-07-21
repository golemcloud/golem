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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::InFunctionRetryHost;
use crate::durable_host::suspendable_wait::{
    ParkOutcome, PromiseWaiting, SuspendableWaitContext, chrono_duration_to_nanos,
    ephemeral_sleep_too_long_error, park_suspendable_wait, std_duration_to_nanos,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, SuspendForSleep};
use crate::workerctx::WorkerCtx;
use chrono::{Duration, Utc};
use futures::pin_mut;
use golem_common::model::Timestamp;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::host_functions::{IoPollPoll, IoPollReady};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestPollCount, HostResponsePollReady,
    HostResponsePollResult,
};
use golem_service_base::error::worker_executor::InterruptKind;
use tracing::debug;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView as _;
use wasmtime_wasi::p2::bindings::io::poll::{Host, HostPollable, Pollable};

impl<Ctx: WorkerCtx> HostPollable for DurableWorkerCtx<Ctx> {
    async fn ready(&mut self, self_: Resource<Pollable>) -> wasmtime::Result<bool> {
        self.observe_function_call("io::poll:pollable", "ready");
        let rep = self_.rep();
        let handle = CallHandle::<IoPollReady, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let was_live = handle.is_live();

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let result = {
                    let mut view = ctx.as_wasi_view();
                    HostPollable::ready(&mut view.io_data(), self_)
                        .await
                        .map_err(|err| err.to_string())
                };
                Ok(HostResponsePollReady { result })
            })
            .await?;

        let is_ready = result.result.map_err(wasmtime::Error::msg)?;

        // A file-stream pollable recorded as ready was actually awaited by the live run before
        // its result was persisted, and the file operation it gates re-executes for real during
        // replay. Await the real readiness too, so subsequent (non-durable) reads/writes observe
        // the same stream state as the recorded run (see
        // `PrivateDurableWorkerState::file_stream_pollables`).
        if !was_live && is_ready && self.state.file_stream_pollables.contains(&rep) {
            let pollable = Resource::<Pollable>::new_borrow(rep);
            let mut view = self.as_wasi_view();
            HostPollable::block(&mut view.io_data(), pollable).await?;
        }

        Ok(is_ready)
    }

    async fn block(&mut self, self_: Resource<Pollable>) -> wasmtime::Result<()> {
        self.observe_function_call("io::poll:pollable", "block");
        let in_ = vec![self_];
        let _ = self.poll(in_).await?;

        Ok(())
    }

    fn drop(&mut self, rep: Resource<Pollable>) -> wasmtime::Result<()> {
        self.observe_function_call("io::poll:pollable", "drop");
        let child_rep = rep.rep();

        // Check if this pollable is a child of a FutureInvokeResult
        let parent_rep = self.state.rpc_pollable_to_parent.get(&child_rep).copied();

        {
            let mut view = self.as_wasi_view();
            HostPollable::drop(&mut view.io_data(), rep)?;
        }

        // Only unclassify after the resource is really gone: reps are recycled by the
        // resource table, and a failed drop leaves the pollable live.
        self.state.file_stream_pollables.remove(&child_rep);

        // If this child belonged to a FutureInvokeResult whose drop was deferred,
        // finalize the parent deletion now that this child is gone.
        if let Some(parent_rep) = parent_rep {
            self.state.rpc_pollable_to_parent.remove(&child_rep);
            let parent: Resource<crate::durable_host::wasm_rpc::FutureInvokeResultEntry> =
                Resource::new_borrow(parent_rep);
            let should_delete = if let Ok(entry) = self.table().get_mut(&parent) {
                entry.child_pollables.retain(|r| *r != child_rep);
                entry.drop_pending && entry.child_pollables.is_empty()
            } else {
                false
            };

            if should_delete {
                let parent_owned: Resource<crate::durable_host::wasm_rpc::FutureInvokeResultEntry> =
                    Resource::new_own(parent_rep);
                if let Err(err) = self.table().delete(parent_owned) {
                    debug!(
                        parent_rep,
                        error = %err,
                        "Deferred future invoke result delete failed"
                    );
                }
            }
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    /// Mixed-ABI suspension semantics: the worker may only be suspended from a P2 `poll` when
    /// every in-flight live durable host call is parked in a recognized suspendable wait
    /// ([`PrivateDurableWorkerState::safe_to_suspend`](crate::durable_host::PrivateDurableWorkerState)).
    /// Pending P3 host work in the same store (e.g. an in-flight `wasi:http` send spawned by
    /// another guest task) therefore blocks suspension: suspending would drop the pending host
    /// future, leave its `Start` entry incomplete and force the side effect to be re-executed on
    /// resume. Instead, a long P2 sleep parks in a suspendable wait — mirroring the P3
    /// `monotonic-clock` waits — and either suspends once it becomes safe, or, if the sleep
    /// deadline is reached first, re-runs the poll without ever suspending.
    async fn poll(&mut self, in_: Vec<Resource<Pollable>>) -> wasmtime::Result<Vec<u32>> {
        // check if all pollables are promise backed. In this case we can suspend immediately
        // This check only needs to be done in live mode, as we will never even persist the oplog entry for polling
        // if we suspended in the last pass. Doing it this way also prevents us from initializing the promises until we are actually in live mode.
        //
        // The immediate suspension is additionally gated on `safe_to_suspend()`: pending live
        // host work (e.g. an in-flight P3 HTTP send) must not be dropped by suspending. Note
        // that `promise_backed_pollables` currently has no insertion sites (P2 promise pollables
        // were superseded by the P3 promise-result API), so this fast path is unreachable for
        // non-empty poll lists. If such registrations are ever reintroduced, this path must be
        // turned into a suspendable-wait park (like the P3 promise wait) instead of skipping
        // suspension entirely, so that a poll blocked only on promises still suspends once
        // pending host work completes.
        if self.durable_execution_state().is_live
            && self.agent_mode() != AgentMode::Ephemeral
            && self.state.safe_to_suspend()
        {
            let promise_backed_pollables = self.state.promise_backed_pollables.read().await;
            let mut all_blocked = true;

            for res in &in_ {
                if let Some(promise_handle) = promise_backed_pollables.get(&res.rep()) {
                    let ready = promise_handle.is_ready().await;
                    if ready {
                        all_blocked = false;
                        break;
                    }
                } else {
                    all_blocked = false;
                    break;
                }
            }

            if all_blocked {
                debug!("Suspending worker until a promise gets completed");
                return Err(wasmtime::Error::from_anyhow(
                    InterruptKind::Suspend(Timestamp::now_utc()).into(),
                ));
            }
        };

        let count = in_.len();
        let mut handle = CallHandle::<IoPollPoll, NotCancellable>::start(
            self,
            HostRequestPollCount { count },
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let response: HostResponsePollResult = 'poll: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        // File-stream pollables recorded as ready were actually awaited by the
                        // live run before this poll's result was persisted, and the file
                        // operations they gate re-execute for real during replay. Await their
                        // real readiness too, so subsequent (non-durable) reads/writes observe
                        // the same stream state as the recorded run (see
                        // `PrivateDurableWorkerState::file_stream_pollables`).
                        if let Ok(ready_indices) = &response.result {
                            let ready_file_pollables = ready_indices
                                .iter()
                                .filter_map(|idx| in_.get(*idx as usize))
                                .map(|pollable| pollable.rep())
                                .filter(|rep| self.state.file_stream_pollables.contains(rep))
                                .collect::<Vec<_>>();
                            for rep in ready_file_pollables {
                                let pollable = Resource::<Pollable>::new_borrow(rep);
                                let mut view = self.as_wasi_view();
                                HostPollable::block(&mut view.io_data(), pollable).await?;
                            }
                        }
                        break 'poll response;
                    }
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let record_ephemeral_promise_wait = if self.agent_mode() == AgentMode::Ephemeral {
                let promise_backed_pollables = self.state.promise_backed_pollables.read().await;
                let mut all_blocked = true;

                for res in &in_ {
                    if let Some(promise_handle) = promise_backed_pollables.get(&res.rep()) {
                        let ready = promise_handle.is_ready().await;
                        if ready {
                            all_blocked = false;
                            break;
                        }
                    } else {
                        all_blocked = false;
                        break;
                    }
                }

                all_blocked && !in_.is_empty()
            } else {
                false
            };
            let ephemeral_poll_timeout = if self.agent_mode() == AgentMode::Ephemeral {
                Some(self.state.config.suspend.ephemeral_max_sleep)
            } else {
                None
            };

            // The poll may need to be re-executed after parking on a suspend-for-sleep (see
            // below), but `Host::poll` consumes its pollable arguments, so keep the raw reps
            // around to recreate the borrows for retries.
            let reps = in_.iter().map(|res| res.rep()).collect::<Vec<_>>();
            let mut pollables = Some(in_);

            loop {
                let in_ = pollables.take().unwrap_or_else(|| {
                    reps.iter()
                        .map(|rep| Resource::new_borrow(*rep))
                        .collect::<Vec<_>>()
                });

                let interrupt_signal = self
                    .execution_status
                    .read()
                    .unwrap()
                    .create_await_interrupt_signal();

                let result = {
                    let mut view = self.as_wasi_view();
                    let mut io_data = view.io_data();
                    let poll = Host::poll(&mut io_data, in_);
                    pin_mut!(poll);

                    let _promise_waiting = PromiseWaiting::new(record_ephemeral_promise_wait);

                    if let Some(timeout_duration) = ephemeral_poll_timeout {
                        let timeout = tokio::time::sleep(timeout_duration);
                        pin_mut!(timeout);

                        tokio::select! {
                            result = &mut poll => {
                                result
                            }
                            interrupt_kind = interrupt_signal => {
                                // Trap leaves the eager host-call `Start` incomplete (re-executed on
                                // replay); never written as a `Cancelled`.
                                handle.abandon_for_trap();
                                return Err(wasmtime::Error::from_anyhow(interrupt_kind.into()));
                            }
                            _ = &mut timeout => {
                                let max_nanos = std_duration_to_nanos(timeout_duration);
                                return Err(wasmtime::Error::from_anyhow(
                                    handle.trap(ephemeral_sleep_too_long_error(max_nanos, max_nanos)),
                                ));
                            }
                        }
                    } else {
                        tokio::select! {
                            result = &mut poll => {
                                result
                            }
                            interrupt_kind = interrupt_signal => {
                                handle.abandon_for_trap();
                                return Err(wasmtime::Error::from_anyhow(interrupt_kind.into()));
                            }
                        }
                    }
                };

                match is_suspend_for_sleep(&result) {
                    Some(duration) => {
                        if self.agent_mode() == AgentMode::Ephemeral {
                            let max = self.state.config.suspend.ephemeral_max_sleep;
                            return Err(wasmtime::Error::from_anyhow(handle.trap(
                                ephemeral_sleep_too_long_error(
                                    chrono_duration_to_nanos(duration),
                                    std_duration_to_nanos(max),
                                ),
                            )));
                        }

                        // Do not suspend the worker right away: unrelated P3 host work (e.g. an
                        // in-flight `wasi:http` send from another guest task) may be pending in
                        // the same store, and suspending would drop it mid-flight. Park in a
                        // suspendable wait instead — while parked, the store's event loop keeps
                        // driving pending host futures. The worker is only suspended once every
                        // live host call is parked in such a wait; if the sleep deadline arrives
                        // first, the poll is simply re-executed.
                        let deadline = tokio::time::Instant::now()
                            + duration.to_std().unwrap_or(std::time::Duration::ZERO);
                        let context = SuspendableWaitContext {
                            wait_id: self.state.next_suspendable_wait_id(),
                            agent_mode: self.agent_mode(),
                            suspend: self.state.config.suspend.clone(),
                            wait_deadline: Some(Utc::now() + duration),
                            suspendable_waits: self.state.suspendable_waits(),
                            wakeup_scheduler: self.state.wakeup_scheduler(),
                        };
                        let outcome = park_suspendable_wait(
                            context,
                            self.create_interrupt_signal(),
                            || tokio::time::sleep_until(deadline),
                            || tokio::time::Instant::now() >= deadline,
                            || self.state.safe_to_suspend(),
                            || {
                                Some(
                                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                                )
                            },
                        )
                        .await
                        .map_err(|err| wasmtime::Error::from_anyhow(handle.trap(err)))?;

                        match outcome {
                            ParkOutcome::Ready => {
                                // The sleep deadline was reached while it was not safe to
                                // suspend: re-execute the poll, which now completes without
                                // requesting another suspend-for-sleep.
                            }
                            ParkOutcome::SuspendWorker => {
                                // The worker suspends and re-executes this poll on resume; the
                                // eager `Start` is left incomplete (resolved by incomplete-replay
                                // re-execution), not persisted. The wakeup at the sleep deadline
                                // was already scheduled by the park.
                                handle.abandon_for_trap();
                                return Err(wasmtime::Error::from_anyhow(
                                    InterruptKind::Suspend(Timestamp::now_utc()).into(),
                                ));
                            }
                            ParkOutcome::Interrupted(kind) => {
                                handle.abandon_for_trap();
                                return Err(wasmtime::Error::from_anyhow(kind.into()));
                            }
                            ParkOutcome::EphemeralTooLong {
                                requested_nanos,
                                max_nanos,
                            } => {
                                return Err(wasmtime::Error::from_anyhow(handle.trap(
                                    ephemeral_sleep_too_long_error(requested_nanos, max_nanos),
                                )));
                            }
                        }
                    }
                    None => {
                        break 'poll handle
                            .complete(
                                self,
                                HostResponsePollResult {
                                    result: result.map_err(|err| err.to_string()),
                                },
                            )
                            .await?;
                    }
                }
            }
        };

        response.result.map_err(wasmtime::Error::msg)
    }
}

fn is_suspend_for_sleep<T>(result: &Result<T, wasmtime::Error>) -> Option<Duration> {
    if let Err(err) = result {
        // Walk the error source chain, since wasmtime::Error may wrap the original error
        let mut current: Option<&dyn std::error::Error> = Some(err.as_ref());
        while let Some(e) = current {
            if let Some(SuspendForSleep(duration)) = e.downcast_ref::<SuspendForSleep>() {
                return Some(Duration::from_std(*duration).unwrap());
            }
            current = e.source();
        }
        None
    } else {
        None
    }
}
