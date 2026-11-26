use super::*;
use golem_common::model::InvocationId;

type LatestWorkerStatus = (
    AgentStatus,
    Option<OplogErrorKind>,
    HashMap<OplogIndex, RetryPolicyState>,
    HashMap<OplogIndex, Timestamp>,
    Option<RetryConfig>,
    Option<InvocationId>,
);

/// Folds `entries` into the agent's execution status. An invocation whose result awaits
/// settlement keeps the agent running until its `AgentInvocationSettled`; the last element of
/// the result is that invocation (`AgentStatusRecord::settling_invocation`).
pub(super) fn calculate_latest_worker_status(
    mut current_status: AgentStatus,
    mut last_error_kind: Option<OplogErrorKind>,
    mut current_retry_state: HashMap<OplogIndex, RetryPolicyState>,
    mut current_retry_at: HashMap<OplogIndex, Timestamp>,
    current_retry_policy: Option<RetryConfig>,
    mut settling_invocation: Option<InvocationId>,
    default_retry_policy: &RetryConfig,
    skipped_regions: &DeletedRegions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    terminal_failure_at: &mut Option<Timestamp>,
    terminal_failure_index: &mut Option<OplogIndex>,
) -> LatestWorkerStatus {
    for (idx, entry) in entries {
        // Errors are counted in skipped regions too (but not in deleted ones), so retained
        // retry state survives a Jump.
        // This must happen before the skipped-region continue below.
        if !deleted_regions.is_in_deleted_region(*idx)
            && let OplogEntry::Error {
                retry_from,
                retry_policy_state: Some(state),
                ..
            } = entry
        {
            current_retry_state.insert(*retry_from, state.clone());
        }
        // The backoff deadline of each host call that waits, keyed by its `Start`: the latest
        // retry of the call sets it and the call's terminal ends it, so it never delays the repair
        // of another call, even one sharing the retry point.
        if !deleted_regions.is_in_deleted_region(*idx) {
            match entry {
                OplogEntry::Error {
                    retry_at: Some(retry_at),
                    retrying_start_index: Some(start_index),
                    ..
                } => {
                    current_retry_at.insert(*start_index, *retry_at);
                }
                OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                    current_retry_at.remove(start_index);
                }
                _ => {}
            }
        }

        // Skipping entries in skipped regions, as they are skipped during replay too
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        let was_terminal = matches!(current_status, AgentStatus::Failed | AgentStatus::Exited);
        // For non-skipped errors, update the worker status based on the accumulated retry count
        if !deleted_regions.is_in_deleted_region(*idx)
            && let OplogEntry::Error {
                kind,
                invocation_id,
                error,
                retry_from,
                retry_policy_state,
                ..
            } = entry
        {
            last_error_kind = Some(*kind);
            // A rejection names the invocation it rejects. A rejection that names no invocation
            // is an ordinary trap of the agent, after the invocation's recorded result.
            if *kind == OplogErrorKind::Invocation
                && invocation_id.is_some()
                && matches!(
                    error,
                    AgentError::InvalidRequest(_) | AgentError::PermissionDenied(_)
                )
            {
                current_status = idle_unless_settling(settling_invocation);
                last_error_kind = None;
                current_retry_state.clear();
                current_retry_at.clear();
            } else {
                let count = current_retry_state
                    .get(retry_from)
                    .map(|s| s.retry_count())
                    .unwrap_or_default();
                if (*kind == OplogErrorKind::Recovery && retry_policy_state.is_none())
                    || is_worker_error_retriable(
                        current_retry_policy
                            .as_ref()
                            .unwrap_or(default_retry_policy),
                        error,
                        count,
                        retry_policy_state.as_ref(),
                    )
                {
                    current_status = AgentStatus::Retrying;
                } else {
                    current_status = AgentStatus::Failed;
                }
            }
        }

        let status_before_entry = current_status;
        let unresolved_recovery = last_error_kind == Some(OplogErrorKind::Recovery);

        match entry {
            OplogEntry::Create { .. } => {
                current_status = AgentStatus::Idle;
            }
            OplogEntry::Start { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::End { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Cancelled { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::CompletionDiscarded { .. } | OplogEntry::CompletionDelivered { .. } => {}
            OplogEntry::AgentInvocationStarted { .. } => {
                current_status = AgentStatus::Running;
                if !unresolved_recovery {
                    last_error_kind = None;
                    current_retry_state.clear();
                    current_retry_at.clear();
                }
            }
            OplogEntry::AgentInvocationFinished {
                invocation_id,
                awaits_settlement,
                ..
            }
            | OplogEntry::AgentInvocationFailed {
                invocation_id,
                awaits_settlement,
                ..
            } => {
                if *awaits_settlement {
                    settling_invocation = Some(*invocation_id);
                }
                current_status = idle_unless_settling(settling_invocation);
                if !unresolved_recovery {
                    last_error_kind = None;
                    current_retry_state.clear();
                    current_retry_at.clear();
                }
            }
            OplogEntry::AgentInvocationSettled { invocation_id, .. } => {
                if settling_invocation == Some(*invocation_id) {
                    settling_invocation = None;
                }
                current_status = idle_unless_settling(settling_invocation);
                if !unresolved_recovery {
                    last_error_kind = None;
                    current_retry_state.clear();
                    current_retry_at.clear();
                }
            }
            OplogEntry::Suspend { .. } => {
                current_status = AgentStatus::Suspended;
            }
            OplogEntry::Jump { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Interrupted { .. } => {
                current_status = AgentStatus::Interrupted;
            }
            OplogEntry::Resumed { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Exited { .. } => {
                current_status = AgentStatus::Exited;
            }
            OplogEntry::SetRetryPolicy { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::RemoveRetryPolicy { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::PendingAgentInvocation { .. } => {}
            OplogEntry::PendingUpdate { .. } => {
                if current_status == AgentStatus::Failed {
                    current_status = AgentStatus::Retrying;
                }
            }
            OplogEntry::FailedUpdate { .. } => {}
            OplogEntry::SuccessfulUpdate { .. } => {}
            OplogEntry::GrowMemory { .. } => {}
            OplogEntry::MemorySizeReconciled { .. } => {}
            OplogEntry::CreateResource { .. } => {}
            OplogEntry::DropResource { .. } => {}
            OplogEntry::Log { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Restart { .. } => {
                current_status = AgentStatus::Idle;
            }
            OplogEntry::ActivatePlugin { .. } => {}
            OplogEntry::DeactivatePlugin { .. } => {}
            OplogEntry::Revert { .. } => {}
            OplogEntry::CancelPendingInvocation { .. } => {}
            OplogEntry::BeginRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::PreCommitRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::PreRollbackRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::CommittedRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::RolledBackRemoteTransaction { .. } => {
                current_status = AgentStatus::Running;
            }
            OplogEntry::Snapshot { .. } => {}
            OplogEntry::SnapshotConfirmed { .. } => {}
            OplogEntry::SpanClosed { .. } => {}
            OplogEntry::OplogProcessorCheckpoint { .. } => {}
            OplogEntry::CardEventQueued { .. } => {}
            OplogEntry::CardInstalled { .. } => {}
            OplogEntry::CardInstallFailed { .. } => {}
            OplogEntry::CardDerived { .. } => {}
            OplogEntry::CardTransferStarted { .. } => {}
            OplogEntry::CardTransferred { .. } => {}
            OplogEntry::CardRevokedCascade { .. } => {}
            OplogEntry::CardTransferConfirmed { .. } => {}
            OplogEntry::CardRevoked { .. } => {}
            OplogEntry::CardExpired { .. } => {}
            OplogEntry::HostStreamFrame { .. } => {}
            OplogEntry::ToolAttachmentControl { .. } => {}
            OplogEntry::ToolAttachmentRead { .. } => {}
            OplogEntry::StreamRegistered { .. }
            | OplogEntry::StreamItems { .. }
            | OplogEntry::StreamTerminal { .. }
            | OplogEntry::StreamSession { .. } => {}
            OplogEntry::Error { .. } => {
                // .. handled separately
            }
            OplogEntry::RecoverySucceeded { .. } => {
                if !deleted_regions.is_in_deleted_region(*idx)
                    && last_error_kind == Some(OplogErrorKind::Recovery)
                {
                    if matches!(current_status, AgentStatus::Retrying | AgentStatus::Failed) {
                        current_status = idle_unless_settling(settling_invocation);
                    }
                    last_error_kind = None;
                }
            }
        }

        if unresolved_recovery
            && !matches!(
                entry,
                OplogEntry::Error {
                    kind: OplogErrorKind::Invocation | OplogErrorKind::Recovery,
                    ..
                } | OplogEntry::RecoverySucceeded { .. }
                    | OplogEntry::Suspend { .. }
                    | OplogEntry::Interrupted { .. }
                    | OplogEntry::Exited { .. }
            )
        {
            current_status = status_before_entry;
        }
        if matches!(current_status, AgentStatus::Failed | AgentStatus::Exited) {
            if !was_terminal {
                *terminal_failure_at = Some(entry.timestamp());
                *terminal_failure_index = Some(*idx);
            }
        } else {
            *terminal_failure_at = None;
            *terminal_failure_index = None;
        }
    }
    (
        current_status,
        last_error_kind,
        current_retry_state,
        current_retry_at,
        current_retry_policy,
        settling_invocation,
    )
}

/// The status of an agent that has no running invocation: `Running` while the background work
/// of an invocation whose result awaits settlement drains, `Idle` otherwise.
fn idle_unless_settling(settling_invocation: Option<InvocationId>) -> AgentStatus {
    if settling_invocation.is_some() {
        AgentStatus::Running
    } else {
        AgentStatus::Idle
    }
}

pub(super) fn is_worker_error_retriable(
    retry_config: &RetryConfig,
    error: &AgentError,
    retry_count: u32,
    semantic_retry_state: Option<&RetryPolicyState>,
) -> bool {
    if matches!(error, AgentError::DeterministicTrap(_)) {
        return false;
    }
    if let Some(state) = semantic_retry_state {
        return !state.is_exhausted();
    }

    match error {
        AgentError::Unknown(_) | AgentError::TransientError(_) => {
            retry_count < retry_config.max_attempts
        }
        AgentError::InvalidRequest(_) => false,
        AgentError::StackOverflow => false,
        AgentError::OutOfMemory => true,
        AgentError::ExceededMemoryLimit => false,
        AgentError::ExceededTableLimit => false,
        AgentError::InternalError(_) => false,
        AgentError::DeterministicTrap(_) => false,
        AgentError::PermanentError(_) => false,
        AgentError::ExceededHttpCallLimit => false,
        AgentError::ExceededRpcCallLimit => false,
        AgentError::AgentTerminatedByQuota(_) => false,
        AgentError::EphemeralSleepTooLong(_) => false,
        AgentError::EphemeralFuelExhausted(_) => false,
        AgentError::EphemeralCannotSuspend(_) => false,
        AgentError::ReadOnlyViolation(_) => false,
        AgentError::PermissionDenied(_) => false,
    }
}
