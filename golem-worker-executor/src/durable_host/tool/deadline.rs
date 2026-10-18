// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

//! The collection import owns one guest-delivery boundary. Its recorded Start
//! anchors the deadline; the race is a host-internal result under that boundary's
//! cancellation owner. Replay never reselects it from current readiness.

use super::*;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CompletionDelivery, DeferredCallReplayOutcome, HostInternalCallOwner,
};
use crate::durable_host::durability::find_durable_call_trap_context;
use crate::durable_host::suspendable_wait::{
    ParkOutcome, ephemeral_sleep_too_long_error, wait_for_ready,
};
use crate::services::oplog::CommitLevel;
use golem_common::model::Timestamp;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::host_functions::{
    GolemToolCollectionRace, GolemToolCollectionWait,
};
use golem_common::model::oplog::{
    HostRequestToolCollectionRace, HostRequestToolCollectionWait,
    HostResponseToolCollectionWaitResult, OplogEntry,
};
use std::time::Duration;

pub(super) struct CollectionDeadline {
    call: Option<DurableCallSession<GolemToolCollectionWait, Cancellable>>,
    replayed: Option<(HostResponseToolCollectionWaitResult, CompletionDelivery)>,
    race_owner: Option<HostInternalCallOwner>,
    secret_owner: Option<HostInternalCallOwner>,
    deadline_epoch_millis: u64,
}

impl CollectionDeadline {
    pub(super) async fn start<U: Send + 'static, Ctx: WorkerCtx>(
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        execution: &ToolExecution,
        timeout_millis: u64,
    ) -> anyhow::Result<Self> {
        let request = HostRequestToolCollectionWait {
            invocation_start: execution.start,
            timeout_millis,
        };
        let mut call =
            DurableCallSession::<GolemToolCollectionWait, Cancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::ReadLocal,
                AccessClaimOptions {
                    host_internal_calls: true,
                    ..Default::default()
                },
                request,
            )
            .await?;
        // Start's timestamp is already authoritative, including after a crash
        // before any terminal. Reusing it avoids a separate clock call or a
        // fresh full timeout when only this Start survived reconstruction.
        let started = if call.is_persisted() {
            let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
            match oplog.read(call.start_index()).await {
                Ok(OplogEntry::Start { timestamp, .. }) => timestamp,
                Ok(_) => {
                    return Err(call.trap(anyhow!("collection deadline has no recorded Start")));
                }
                Err(error) => return Err(call.trap(error)),
            }
        } else {
            Timestamp::now_utc()
        };
        let deadline_epoch_millis = started.to_millis().saturating_add(timeout_millis);
        let race_owner = call.host_internal_owner();
        let secret_owner = call.host_internal_owner();
        let (call, replayed) = if call.is_live() {
            (Some(call), None)
        } else {
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(response, delivery) => {
                    (None, Some((response, delivery)))
                }
                DeferredCallReplayOutcome::Incomplete(call) => (Some(call), None),
            }
        };
        Ok(Self {
            call,
            replayed,
            race_owner: Some(race_owner),
            secret_owner: Some(secret_owner),
            deadline_epoch_millis,
        })
    }

    pub(super) async fn wait<U: Send + 'static, Ctx: WorkerCtx>(
        &mut self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        execution: &Arc<ToolExecution>,
    ) -> anyhow::Result<bool> {
        let selected = DurableCallSession::<GolemToolCollectionRace, Cancellable>::invoke_host_internal_access(
            accessor, accessor.getter(), self.race_owner.take().expect("one collection race"),
            self.replayed.is_some(),
            HostRequestToolCollectionRace {
                invocation_start: execution.start,
                deadline_epoch_millis: self.deadline_epoch_millis,
            },
            DurableFunctionType::ReadLocal,
            async || {
                let timed_out = tokio::select! {
                    biased;
                    result = execution.result() => { result?; false },
                    elapsed = wait_until(accessor, self.deadline_epoch_millis) => { elapsed?; true },
                };
                Ok::<_, anyhow::Error>(HostResponseToolCollectionWaitResult { timed_out })
            },
        ).await?;
        if selected.timed_out && execution.cancellable {
            // An internal End has no guest delivery marker, but an append alone
            // is not a durability barrier. Commit the selected winner before
            // cancellation can alter the tool or any attachment.
            let worker = accessor.with(|mut access| {
                let ctx = access.get();
                (!ctx.state.durability_is_suppressed()).then(|| ctx.public_state.worker())
            });
            commit_then_cancel(
                async {
                    if let Some(worker) = worker {
                        worker
                            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                            .await?;
                    }
                    Ok(())
                },
                || execution.cancel(),
            )
            .await?;
        }
        Ok(selected.timed_out)
    }

    pub(super) fn secret_owner(&mut self) -> (HostInternalCallOwner, bool) {
        (
            self.secret_owner.take().expect("one response admission"),
            self.replayed.is_some(),
        )
    }

    pub(super) async fn finish<U: Send + 'static, Ctx: WorkerCtx>(
        mut self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        timed_out: bool,
    ) -> anyhow::Result<()> {
        let delivery = if let Some((response, delivery)) = self.replayed.take() {
            if response.timed_out != timed_out {
                delivery.suppress();
                return Err(anyhow!("recorded collection and deadline winner disagree"));
            }
            delivery
        } else {
            let (_, delivery) = self
                .call
                .take()
                .expect("live collection retains its session")
                .complete_access_deferred(
                    accessor,
                    accessor.getter(),
                    HostResponseToolCollectionWaitResult { timed_out },
                )
                .await?;
            delivery
        };
        // All stream observations and host-internal secret admission precede
        // the sole guest-terminal observer.
        delivery.deliver_at_accessor_terminal(accessor).await?;
        Ok(())
    }

    pub(super) fn trap(mut self, error: anyhow::Error) -> anyhow::Error {
        if let Some(mut call) = self.call.take() {
            if find_durable_call_trap_context(&error).is_some() {
                // The inner clock/permission/result call already names its
                // retry scope. Abandon this owner without replacing that scope.
                call.abandon_for_trap();
                error
            } else {
                call.trap(error)
            }
        } else {
            if let Some((_, delivery)) = self.replayed.take() {
                delivery.suppress();
            }
            error
        }
    }
}

async fn commit_then_cancel(
    commit: impl std::future::Future<Output = anyhow::Result<()>>,
    cancel: impl FnOnce(),
) -> anyhow::Result<()> {
    commit.await?;
    cancel();
    Ok(())
}

async fn wait_until<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    deadline_epoch_millis: u64,
) -> anyhow::Result<()> {
    // The stored wall-clock deadline survives Store replacement. The resident
    // wait is monotonic; reconstruct only its remaining duration, not a fresh
    // full timeout. The recorded race, not a later clock read, selects replay.
    let remaining = Duration::from_millis(
        deadline_epoch_millis.saturating_sub(Timestamp::now_utc().to_millis()),
    );
    let (mode, max_sleep, runtime, interrupt) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.agent_mode(),
            ctx.state.config.suspend.ephemeral_max_sleep,
            (ctx.agent_mode() == AgentMode::Durable && !ctx.state.durability_is_suppressed())
                .then(|| ctx.runtime_suspension.clone())
                .flatten(),
            ctx.create_interrupt_signal(),
        )
    });
    if let Some(runtime) = runtime {
        let when = std::time::Instant::now()
            .checked_add(remaining)
            .ok_or_else(|| anyhow!("tool collection deadline exceeds the clock range"))?;
        let activity = accessor
            .runtime_activity()
            .and_then(|id| runtime.timer(id, when))
            .ok_or_else(|| anyhow!("tool collection timer runtime activity is not registered"))?;
        return activity
            .wait(tokio::time::sleep_until(when.into()), interrupt)
            .await
            .map_err(Into::into);
    }
    match wait_for_ready(
        mode,
        max_sleep,
        // The tool may finish before a distant timeout. Apply the ephemeral
        // wait limit to time actually spent waiting, not the requested deadline.
        None,
        interrupt,
        tokio::time::sleep(remaining),
    )
    .await
    {
        ParkOutcome::Ready => Ok(()),
        ParkOutcome::Interrupted(kind) => Err(kind.into()),
        ParkOutcome::EphemeralTooLong {
            requested_nanos,
            max_nanos,
        } => Err(ephemeral_sleep_too_long_error(requested_nanos, max_nanos).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::commit_then_cancel;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;

    #[test]
    async fn deadline_cancellation_waits_for_the_durable_commit() {
        let cancelled = AtomicBool::new(false);
        let (committed, barrier) = tokio::sync::oneshot::channel();
        let mut selected = Box::pin(commit_then_cancel(
            async {
                barrier.await?;
                Ok(())
            },
            || cancelled.store(true, Ordering::SeqCst),
        ));
        assert!(futures::poll!(selected.as_mut()).is_pending());
        assert!(!cancelled.load(Ordering::SeqCst));
        committed.send(()).unwrap();
        selected.await.unwrap();
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    async fn failed_deadline_commit_cannot_cancel_the_tool() {
        let cancelled = AtomicBool::new(false);
        let result = commit_then_cancel(async { Err(anyhow::anyhow!("commit failed")) }, || {
            cancelled.store(true, Ordering::SeqCst)
        })
        .await;
        assert!(result.is_err());
        assert!(!cancelled.load(Ordering::SeqCst));
    }
}
