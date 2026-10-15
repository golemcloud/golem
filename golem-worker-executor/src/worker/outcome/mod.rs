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

use super::interrupt::{InterruptGuard, Interrupts};
use super::{
    InvocationFailureOrigin, InvocationOutcomeVerdict, InvocationOutcomeWrite,
    SelectedInvocationOutcome, StopProgress, WorkerInstance, WorkerInterruptState,
};
use crate::model::TrapType;
use crate::services::golem_config::GolemConfig;
use crate::services::resource_limits::MonthlyResourceExhaustion;
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::AgentStatus;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::{IdempotencyKey, OplogIndex};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use uuid::Uuid;

#[cfg(test)]
mod tests;

pub(super) struct OutcomeContext {
    pub generation: u64,
    pub key: Option<IdempotencyKey>,
    pub mode: AgentMode,
}

/// Capture ownership before stopping consumes the Running metadata. An unchanged counter
/// alone does not identify a pre-owner attempt as the previous generation's owner.
pub(super) fn started_marker_generation(
    lifecycle: &WorkerInstance,
    expected_attempt: Uuid,
    generation: u64,
) -> Option<u64> {
    matches!(lifecycle.deletion_runtime(), WorkerInstance::Running(running)
        if running.owner_generation_started && running.start_attempt == expected_attempt)
    .then_some(generation)
}

/// A committed lifecycle writer can satisfy a later Suspend of its established owner.
/// Its exact cause remains distinct from the accepted request's cause and writer claim.
pub(super) fn unloaded_lifecycle_marker(
    kind: InterruptKind,
    status: AgentStatus,
    generation: u64,
    established_generation: Option<u64>,
    selected: Option<&SelectedInvocationOutcome>,
) -> Result<Option<OplogEntry>, WorkerExecutorError> {
    match kind {
        InterruptKind::Suspend(_) => {
            if status == AgentStatus::Suspended
                && established_generation == Some(generation)
                && let Some(selected) = selected.filter(|selected| {
                    selected.generation == generation
                        && selected.writer_claimed
                        && matches!(
                            &selected.failure,
                            Some(TrapType::Interrupt(InterruptKind::Suspend(_)))
                        )
                })
                && let Some(completion) = &selected.writer_completion
            {
                return match completion.clone().now_or_never() {
                    Some(Ok(InvocationOutcomeVerdict::Committed)) => Ok(None),
                    Some(Ok(InvocationOutcomeVerdict::Refused(fence))) => {
                        Err(crate::services::oplog::OplogError::Fenced(fence).into())
                    }
                    Some(Err(error)) => Err(error),
                    None => Err(WorkerExecutorError::runtime(
                        "Lifecycle marker writer is still pending",
                    )),
                };
            }
            Ok(Some(OplogEntry::suspend()))
        }
        InterruptKind::Interrupt(_) => Ok(Some(OplogEntry::interrupted())),
        InterruptKind::Restart | InterruptKind::Jump | InterruptKind::ShardLost => Ok(None),
    }
}

pub(super) async fn lock_for_outcome<'a>(
    interrupts: &'a Interrupts,
    progress: &StopProgress,
) -> Result<InterruptGuard<'a>, WorkerExecutorError> {
    loop {
        let guard = interrupts.lock().await;
        if !matches!(&*guard,
            WorkerInterruptState::Unpublished(pending) | WorkerInterruptState::Freezing(pending)
            if pending.is_terminal())
        {
            return Ok(guard);
        }
        let (publication, completion) = progress
            .lock()
            .unwrap()
            .publication
            .clone()
            .expect("accepted stop has retained publication");
        let notified = publication.published.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        drop(guard);
        tokio::select! {
            _ = notified => {}
            result = completion => { result?; }
        }
    }
}

pub(super) async fn lock_for_failure<'a>(
    interrupts: &'a Interrupts,
    progress: &StopProgress,
    origin: InvocationFailureOrigin,
) -> (InterruptGuard<'a>, Option<WorkerExecutorError>) {
    match origin {
        InvocationFailureOrigin::Independent => (interrupts.lock().await, None),
        InvocationFailureOrigin::InvocationDeadline | InvocationFailureOrigin::Lifecycle => {
            match lock_for_outcome(interrupts, progress).await {
                Ok(guard) => (guard, None),
                Err(error) => (interrupts.lock().await, Some(error)),
            }
        }
    }
}

pub(super) fn claim_success<Ctx: WorkerCtx>(
    selected: &mut Option<SelectedInvocationOutcome>,
    interrupts: &mut InterruptGuard<'_>,
    context: OutcomeContext,
    monthly_error: impl Fn(InterruptKind) -> Option<WorkerExecutorError>,
) -> Result<InvocationOutcomeWrite, WorkerExecutorError> {
    let OutcomeContext {
        generation,
        key,
        mode,
    } = context;
    if selected
        .as_ref()
        .is_some_and(|selected| selected.generation == generation && selected.key == key)
    {
        return Err(WorkerExecutorError::runtime(
            "Invocation outcome was already selected",
        ));
    }
    let pending = interrupts.claim_pending_terminal();
    let error = pending.map(|pending| {
        monthly_error(pending.kind)
            .unwrap_or(WorkerExecutorError::Interrupted { kind: pending.kind })
    });
    let failure = error.clone().map(|error| match error {
        WorkerExecutorError::Interrupted { kind } => TrapType::Interrupt(kind),
        error => TrapType::from_worker_executor_error::<Ctx>(
            error,
            OplogIndex::INITIAL,
            false,
            false,
            mode,
        ),
    });
    let (writer, completion) = InvocationOutcomeWrite::new();
    *selected = Some(SelectedInvocationOutcome {
        generation,
        key,
        failure,
        writer_claimed: error.is_none(),
        writer_completion: error.is_none().then_some(completion),
    });
    error.map_or(Ok(writer), Err)
}

pub(super) fn claim_failure<Ctx: WorkerCtx>(
    selected: &mut Option<SelectedInvocationOutcome>,
    interrupts: &mut InterruptGuard<'_>,
    context: OutcomeContext,
    trap: &TrapType,
    origin: InvocationFailureOrigin,
    publication_error: Option<WorkerExecutorError>,
    monthly_error: impl Fn(InterruptKind) -> Option<WorkerExecutorError>,
) -> Option<(TrapType, InvocationOutcomeWrite)> {
    let OutcomeContext {
        generation,
        key,
        mode,
    } = context;
    if let Some(selected) = selected
        .as_mut()
        .filter(|selected| selected.generation == generation && selected.key == key)
    {
        if selected.writer_claimed {
            return None;
        }
        let failure = selected.failure.clone()?;
        let (writer, completion) = InvocationOutcomeWrite::new();
        selected.writer_claimed = true;
        selected.writer_completion = Some(completion);
        return Some((failure, writer));
    }
    let pending = interrupts.claim_pending_terminal();
    let elected_error = publication_error.or_else(|| {
        matches!(
            origin,
            InvocationFailureOrigin::InvocationDeadline | InvocationFailureOrigin::Lifecycle
        )
        .then_some(pending)
        .flatten()
        .map(|pending| {
            monthly_error(pending.kind)
                .unwrap_or(WorkerExecutorError::Interrupted { kind: pending.kind })
        })
    });
    let trap = match (elected_error, trap) {
        (Some(WorkerExecutorError::Interrupted { kind }), _) => TrapType::Interrupt(kind),
        (Some(error), _) => TrapType::from_worker_executor_error::<Ctx>(
            error,
            OplogIndex::INITIAL,
            false,
            false,
            mode,
        ),
        (None, TrapType::Interrupt(kind)) => monthly_error(*kind)
            .map(|error| {
                TrapType::from_worker_executor_error::<Ctx>(
                    error,
                    OplogIndex::INITIAL,
                    false,
                    false,
                    mode,
                )
            })
            .unwrap_or_else(|| trap.clone()),
        _ => trap.clone(),
    };
    let (writer, completion) = InvocationOutcomeWrite::new();
    *selected = Some(SelectedInvocationOutcome {
        generation,
        key,
        failure: Some(trap.clone()),
        writer_claimed: true,
        writer_completion: Some(completion),
    });
    Some((trap, writer))
}

pub(super) fn monthly_interrupt_error(
    kind: InterruptKind,
    mode: AgentMode,
    accepted: &Option<(InterruptKind, MonthlyResourceExhaustion)>,
    config: &GolemConfig,
) -> Option<WorkerExecutorError> {
    if mode != AgentMode::Ephemeral {
        return None;
    }
    accepted.as_ref().and_then(|(selected, exhaustion)| {
        (*selected == kind).then(|| {
            super::invocation_loop::monthly_resource_exhausted_invocation_error(config, *exhaustion)
        })
    })
}
