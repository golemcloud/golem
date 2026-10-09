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

use crate::durable_host::concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
use crate::durable_host::suspendable_wait::{
    chrono_duration_to_nanos, ephemeral_sleep_too_long_error, std_duration_to_nanos,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, SuspendForSleep};
#[cfg(feature = "test-utils")]
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use chrono::Duration;
use futures::pin_mut;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::host_functions::{IoPollPoll, IoPollReady};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestPollCount, HostResponsePollReady,
    HostResponsePollResult,
};
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use std::pin::Pin;
use tracing::debug;
use wasmtime::component::{Accessor, Linker, Resource, ResourceType};
use wasmtime_wasi::IoView as _;
use wasmtime_wasi::p2::bindings::io::poll::{Host, HostPollable, Pollable};

pub(crate) fn add_to_linker<Ctx: WorkerCtx>(
    linker: &mut Linker<Ctx>,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<()> {
    let mut instance = linker.instance("wasi:io/poll@0.2.6")?;
    instance.resource(
        "pollable",
        ResourceType::host::<Pollable>(),
        move |mut store, rep| HostPollable::drop(get(store.data_mut()), Resource::new_own(rep)),
    )?;
    instance.func_wrap_async("[method]pollable.ready", move |mut store, (pollable,)| {
        Box::new(async move { Ok((HostPollable::ready(get(store.data_mut()), pollable).await?,)) })
    })?;
    instance.func_wrap_dispatch(
        "[method]pollable.block",
        move |ctx, (pollable,): &(Resource<Pollable>,)| {
            Ok(owned_timers(get(ctx), std::slice::from_ref(pollable)))
        },
        move |mut store, (pollable,)| {
            Box::new(async move { HostPollable::block(get(store.data_mut()), pollable).await })
        },
        move |accessor, (pollable,)| {
            Box::pin(async move {
                poll_timers(accessor, get, vec![pollable]).await?;
                Ok(())
            })
        },
    )?;
    instance.func_wrap_dispatch(
        "poll",
        move |ctx, (pollables,): &(Vec<Resource<Pollable>>,)| Ok(owned_timers(get(ctx), pollables)),
        move |mut store, (pollables,)| {
            Box::new(async move { Ok((Host::poll(get(store.data_mut()), pollables).await?,)) })
        },
        move |accessor, (pollables,)| {
            Box::pin(async move { Ok((poll_timers(accessor, get, pollables).await?,)) })
        },
    )?;
    Ok(())
}

fn owned_timers<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    pollables: &[Resource<Pollable>],
) -> bool {
    ctx.agent_mode() == AgentMode::Durable
        && !ctx.state.durability_is_suppressed()
        && ctx.runtime_suspension.is_some()
        && !pollables.is_empty()
        && pollables
            .iter()
            .all(|p| ctx.state.p2_timer_deadlines.contains_key(&p.rep()))
}

async fn poll_timers<Ctx: WorkerCtx>(
    accessor: &Accessor<Ctx>,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
    pollables: Vec<Resource<Pollable>>,
) -> wasmtime::Result<Vec<u32>> {
    let response = DurableCallSession::<IoPollPoll, NotCancellable>::invoke_access(
        accessor,
        get,
        HostRequestPollCount {
            count: pollables.len(),
        },
        DurableFunctionType::ReadLocal,
        async || {
            let (deadlines, runtime, interrupt) = accessor.with(|mut access| {
                let ctx = get(access.data_mut());
                let deadlines = pollables
                    .iter()
                    .map(|pollable| {
                        ctx.table().get(pollable)?;
                        ctx.state
                            .p2_timer_deadlines
                            .get(&pollable.rep())
                            .copied()
                            .ok_or_else(|| wasmtime::Error::msg("timer pollable is not registered"))
                    })
                    .collect::<wasmtime::Result<Vec<_>>>()?;
                Ok::<_, wasmtime::Error>((
                    deadlines,
                    ctx.runtime_suspension.clone().unwrap(),
                    ctx.create_interrupt_signal(),
                ))
            })?;
            let deadline = *deadlines.iter().min().unwrap();
            let activity = accessor
                .runtime_activity()
                .and_then(|id| runtime.timer(id, deadline))
                .ok_or_else(|| wasmtime::Error::msg("timer runtime activity is not registered"))?;
            if deadline <= std::time::Instant::now() {
                tokio::task::yield_now().await;
            }
            activity
                .wait(tokio::time::sleep_until(deadline.into()), interrupt)
                .await
                .map_err(|kind| wasmtime::Error::from_anyhow(kind.into()))?;
            let now = std::time::Instant::now();
            Ok::<_, wasmtime::Error>(HostResponsePollResult {
                result: Ok(deadlines
                    .iter()
                    .enumerate()
                    .filter_map(|(index, deadline)| (*deadline <= now).then_some(index as u32))
                    .collect()),
            })
        },
    )
    .await?;
    response.result.map_err(wasmtime::Error::msg)
}

impl<Ctx: WorkerCtx> HostPollable for DurableWorkerCtx<Ctx> {
    async fn ready(&mut self, self_: Resource<Pollable>) -> wasmtime::Result<bool> {
        self.observe_function_call("io::poll:pollable", "ready");
        let rep = self_.rep();
        let handle = DurableCallSession::<IoPollReady, NotCancellable>::start(
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
        // `PrivateDurableWorkerState::file_stream_pollables`). TCP connect likewise re-executes
        // natively, so its recorded readiness must precede synchronous `finish_connect`.
        if !was_live && is_ready {
            if self.state.file_stream_pollables.contains(&rep) {
                let pollable = Resource::<Pollable>::new_borrow(rep);
                let interrupt = self.create_interrupt_signal();
                let mut view = self.as_wasi_view();
                crate::wasi_filesystem::observe_filesystem_operation(
                    interrupt,
                    HostPollable::block(&mut view.io_data(), pollable),
                )
                .await
                .map_err(wasmtime::Error::from)??;
            } else if self.state.tcp_connect_replay.needs_readiness(rep) {
                self.revalidate_tcp_connect(rep, true).await?;
            }
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
        self.state.tcp_connect_replay.drop_pollable(child_rep);
        self.state.p2_timer_deadlines.remove(&child_rep);

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
    async fn poll(&mut self, in_: Vec<Resource<Pollable>>) -> wasmtime::Result<Vec<u32>> {
        let count = in_.len();
        let mut handle = DurableCallSession::<IoPollPoll, NotCancellable>::start(
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
                        // `PrivateDurableWorkerState::file_stream_pollables`). A reconstructed
                        // TCP connect also needs its native future driven before `finish_connect`.
                        // Other network pollables do not share that reconstruction contract.
                        if let Ok(ready_indices) = &response.result {
                            let ready_reconstructed_pollables = ready_indices
                                .iter()
                                .filter_map(|idx| in_.get(*idx as usize))
                                .map(|pollable| pollable.rep())
                                .filter(|rep| {
                                    self.state.file_stream_pollables.contains(rep)
                                        || self.state.tcp_connect_replay.needs_readiness(*rep)
                                })
                                .collect::<Vec<_>>();
                            for rep in ready_reconstructed_pollables {
                                if self.state.file_stream_pollables.contains(&rep) {
                                    let pollable = Resource::<Pollable>::new_borrow(rep);
                                    let interrupt = self.create_interrupt_signal();
                                    let mut view = self.as_wasi_view();
                                    crate::wasi_filesystem::observe_filesystem_operation(
                                        interrupt,
                                        HostPollable::block(&mut view.io_data(), pollable),
                                    )
                                    .await
                                    .map_err(wasmtime::Error::from)??;
                                } else {
                                    self.revalidate_tcp_connect(rep, false).await?;
                                }
                            }
                        }
                        break 'poll response;
                    }
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let ephemeral_poll_timeout = if self.agent_mode() == AgentMode::Ephemeral {
                Some(self.state.config.suspend.ephemeral_max_sleep)
            } else {
                None
            };

            let interrupt_signal = self.create_interrupt_signal();
            #[cfg(feature = "test-utils")]
            let observer = self.public_state.worker().p2_poll_observer_for_test(
                self.state.get_current_idempotency_key(),
                handle.start_index(),
                in_.iter().map(|res| res.rep()).collect(),
            );
            let result = {
                let mut view = self.as_wasi_view();
                let mut io_data = view.io_data();
                let poll = Host::poll(&mut io_data, in_);
                #[cfg(feature = "test-utils")]
                let poll = async {
                    match observer {
                        Some(observer) => observer.observe(poll).await,
                        None => poll.await,
                    }
                };
                poll_borrowed(poll, interrupt_signal, ephemeral_poll_timeout).await
            };
            let result = match result {
                Ok(result) => result,
                Err(BorrowedPollTrap::Interrupted(kind)) => {
                    handle.abandon_for_trap();
                    return Err(wasmtime::Error::from_anyhow(kind.into()));
                }
                Err(BorrowedPollTrap::EphemeralTimeout(max)) => {
                    let nanos = std_duration_to_nanos(max);
                    return Err(wasmtime::Error::from_anyhow(
                        handle.trap(ephemeral_sleep_too_long_error(nanos, nanos)),
                    ));
                }
            };

            if let Some(duration) = is_suspend_for_sleep(&result) {
                let max = self.state.config.suspend.ephemeral_max_sleep;
                return Err(wasmtime::Error::from_anyhow(handle.trap(
                    ephemeral_sleep_too_long_error(
                        chrono_duration_to_nanos(duration),
                        std_duration_to_nanos(max),
                    ),
                )));
            }
            break 'poll handle
                .complete(
                    self,
                    HostResponsePollResult {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?;
        };

        response.result.map_err(wasmtime::Error::msg)
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// The recorded call is already closed. Only its reconstructed native connect can be
    /// interrupted here; replay resolution and its mandatory completion are never raced.
    async fn revalidate_tcp_connect(
        &mut self,
        rep: u32,
        _ready_probe: bool,
    ) -> wasmtime::Result<()> {
        #[cfg(feature = "test-utils")]
        let (wait_gate, outcome_gate) = {
            let gate = self
                .public_state
                .worker()
                .take_p2_connect_replay_gate_for_test(_ready_probe);
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                let _ = gate.subscribe.await;
                (
                    Some((gate.waiting, gate.ready)),
                    Some((gate.selected, gate.finish)),
                )
            } else {
                (None, None)
            }
        };
        let interrupt_signal = self
            .execution_status
            .read()
            .unwrap()
            .create_await_interrupt_signal();
        let result = {
            let readiness = async {
                #[cfg(feature = "test-utils")]
                if let Some((waiting, ready)) = wait_gate {
                    let _ = waiting.send(());
                    let _ = ready.await;
                }
                let mut view = self.as_wasi_view();
                HostPollable::block(&mut view.io_data(), Resource::new_borrow(rep)).await
            };
            tokio::select! {
                result = readiness => result,
                kind = interrupt_signal => Err(wasmtime::Error::from_anyhow(kind.into())),
            }
        };
        #[cfg(feature = "test-utils")]
        if let Some((selected, finish)) = outcome_gate {
            let _ = selected.send(result.is_ok());
            let _ = finish.await;
        }
        result
    }
}

#[derive(Debug)]
enum BorrowedPollTrap {
    Interrupted(InterruptKind),
    EphemeralTimeout(std::time::Duration),
}

async fn poll_borrowed(
    poll: impl Future<Output = wasmtime::Result<Vec<u32>>>,
    interrupt: Pin<Box<dyn Future<Output = InterruptKind> + Send>>,
    ephemeral_timeout: Option<std::time::Duration>,
) -> Result<wasmtime::Result<Vec<u32>>, BorrowedPollTrap> {
    pin_mut!(poll);
    if let Some(max) = ephemeral_timeout {
        tokio::select! {
            result = &mut poll => Ok(result),
            kind = interrupt => Err(BorrowedPollTrap::Interrupted(kind)),
            _ = tokio::time::sleep(max) => Err(BorrowedPollTrap::EphemeralTimeout(max)),
        }
    } else {
        tokio::select! {
            result = &mut poll => Ok(result),
            kind = interrupt => Err(BorrowedPollTrap::Interrupted(kind)),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::time::{Duration as StdDuration, Instant};
    use test_r::test;

    struct Pending;

    #[async_trait::async_trait]
    impl wasmtime_wasi::Pollable for Pending {
        async fn ready(&mut self) {
            pending::<()>().await
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn borrowed_mixed_p2_poll_times_out_ephemeral_but_only_interrupts_durable() {
        let max = StdDuration::from_millis(2);
        let (_, mut io_ctx, mut table) = crate::wasi_host::create_context(
            &[] as &[&str],
            std::io::empty(),
            std::io::empty(),
            std::io::empty(),
            |duration| wasmtime::Error::from(SuspendForSleep(duration)),
            Some(max),
        )
        .unwrap();
        let timer = table.push(Pending).unwrap();
        let timer = wasmtime_wasi::subscribe(
            &mut table,
            timer,
            Some(Instant::now() + StdDuration::from_secs(60)),
        )
        .unwrap();
        let unknown = table.push(Pending).unwrap();
        let unknown = wasmtime_wasi::subscribe(&mut table, unknown, None).unwrap();
        let reps = [timer.rep(), unknown.rep()];
        let mut io = wasmtime_wasi::IoData {
            table: &mut table,
            io_ctx: &mut io_ctx,
        };
        assert!(matches!(poll_borrowed(
            Host::poll(&mut io, reps.iter().map(|rep| Resource::new_borrow(*rep)).collect()),
            Box::pin(pending()), Some(max),
        ).await, Err(BorrowedPollTrap::EphemeralTimeout(duration)) if duration == max));
        assert!(matches!(
            poll_borrowed(
                Host::poll(
                    &mut io,
                    reps.iter().map(|rep| Resource::new_borrow(*rep)).collect()
                ),
                Box::pin(async move {
                    tokio::time::sleep(max * 2).await;
                    InterruptKind::Restart
                }),
                None,
            )
            .await,
            Err(BorrowedPollTrap::Interrupted(InterruptKind::Restart))
        ));
        assert!(matches!(poll_borrowed(
            Host::poll(&mut io, vec![]), Box::pin(pending()), None,
        ).await, Ok(Err(error)) if error.to_string().contains("empty poll list")));
    }
}
