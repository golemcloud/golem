use super::*;

pub(super) fn calculate_latest_worker_status(
    mut current_status: AgentStatus,
    mut last_error_kind: Option<OplogErrorKind>,
    mut current_retry_state: HashMap<OplogIndex, RetryPolicyState>,
    current_retry_policy: Option<RetryConfig>,
    default_retry_policy: &RetryConfig,
    skipped_regions: &DeletedRegions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    AgentStatus,
    Option<OplogErrorKind>,
    HashMap<OplogIndex, RetryPolicyState>,
    Option<RetryConfig>,
) {
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

        // Skipping entries in skipped regions, as they are skipped during replay too
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        // For non-skipped errors, update the worker status based on the accumulated retry count
        if !deleted_regions.is_in_deleted_region(*idx)
            && let OplogEntry::Error {
                kind,
                error,
                retry_from,
                retry_policy_state,
                ..
            } = entry
        {
            last_error_kind = Some(*kind);
            if *kind == OplogErrorKind::Invocation
                && matches!(error, AgentError::PermissionDenied(_))
            {
                current_status = AgentStatus::Idle;
                last_error_kind = None;
                current_retry_state.clear();
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
                }
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                current_status = AgentStatus::Idle;
                if !unresolved_recovery {
                    last_error_kind = None;
                    current_retry_state.clear();
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
            | OplogEntry::StreamEnd { .. }
            | OplogEntry::StreamCancel { .. }
            | OplogEntry::StreamSession { .. } => {}
            OplogEntry::Error { .. } => {
                // .. handled separately
            }
            OplogEntry::RecoverySucceeded { .. } => {
                if !deleted_regions.is_in_deleted_region(*idx)
                    && last_error_kind == Some(OplogErrorKind::Recovery)
                {
                    if matches!(current_status, AgentStatus::Retrying | AgentStatus::Failed) {
                        current_status = AgentStatus::Idle;
                    }
                    last_error_kind = None;
                }
            }
        }

        if unresolved_recovery
            && !matches!(
                entry,
                OplogEntry::Error { .. }
                    | OplogEntry::RecoverySucceeded { .. }
                    | OplogEntry::Suspend { .. }
                    | OplogEntry::Interrupted { .. }
                    | OplogEntry::Exited { .. }
            )
        {
            current_status = status_before_entry;
        }
    }
    (
        current_status,
        last_error_kind,
        current_retry_state,
        current_retry_policy,
    )
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
