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

use super::*;

/// Gives the evidence of a mutation that failed, when the read of its postcondition failed too.
///
/// The read runs only when the error of the mutation does not prove that nothing changed. The
/// evidence is then unknown, and an unknown effect invalidates the generation. So a read that
/// fails needs no check of its own.
pub(super) fn mutation_failure_evidence(error: &FilesystemStorageError) -> EffectEvidence {
    if error_proves_no_effect(error) {
        EffectEvidence::NoEffect
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

pub(super) fn classify_query_error<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    source: FilesystemStorageError,
) -> Error {
    if invalidates_generation(&source) {
        generation.invalidate();
        Error::RuntimeInvalidated
    } else {
        Error::Sandbox(source)
    }
}

pub(super) async fn decide_generation_effect<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    error: &FilesystemStorageError,
    evidence: EffectEvidence,
    budget: RetryBudget,
) -> EffectDecision {
    let mut decision_budget = budget;
    let cause = if error.is_storage_exhaustion() {
        match generation.observe_usage_sandbox().await {
            Ok(FilesystemUsage::Unsupported) => {
                decision_budget = RetryBudget::new(0);
                FailureCause::UnclassifiedIo
            }
            Ok(FilesystemUsage::Authoritative {
                allocated_bytes,
                filesystem_objects,
            }) => {
                let quota_exhausted = match *generation.limits.lock().unwrap() {
                    ResolvedStorageLimits::Unlimited => false,
                    ResolvedStorageLimits::Finite(limits) => {
                        allocated_bytes >= limits.allocated_bytes
                            || filesystem_objects >= limits.filesystem_objects
                    }
                };
                if quota_exhausted {
                    FailureCause::AgentQuota
                } else {
                    decision_budget = RetryBudget::new(0);
                    FailureCause::UnclassifiedIo
                }
            }
            Err(observation_error) if observation_error.is_terminal_failure() => {
                FailureCause::TerminalInfrastructure
            }
            Err(_) => {
                decision_budget = RetryBudget::new(0);
                FailureCause::UnclassifiedIo
            }
        }
    } else {
        classify_failure(error, FailureFacts::default())
    };
    decide_effect(cause, evidence, decision_budget)
}

/// Whether a write that found no space, on storage with `accounting` that reports no usage of
/// the agent, goes to pressure recovery.
///
/// Production storage without per-agent accounting has no agent quota to exhaust, so the volume is
/// full, and pressure recovery can free space. Development storage keeps the failure of the call.
/// Storage with project quotas reports the usage, so it never asks.
fn unreported_usage_goes_to_pressure_recovery(accounting: AgentAccounting) -> bool {
    match accounting {
        AgentAccounting::Unaccounted => true,
        AgentAccounting::ProjectQuotas | AgentAccounting::Development => false,
    }
}

pub(super) async fn decide_write_effect<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    error: &FilesystemStorageError,
    evidence: EffectEvidence,
    budget: RetryBudget,
) -> EffectDecision {
    if !error.is_storage_exhaustion() {
        return decide_effect(
            classify_failure(error, FailureFacts::default()),
            evidence,
            budget,
        );
    }

    let quota_exhausted = match generation.observe_usage_sandbox().await {
        Ok(FilesystemUsage::Unsupported)
            if unreported_usage_goes_to_pressure_recovery(generation.accounting) =>
        {
            false
        }
        Ok(FilesystemUsage::Unsupported) => {
            return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo);
        }
        Ok(FilesystemUsage::Authoritative {
            allocated_bytes,
            filesystem_objects,
        }) => match *generation.limits.lock().unwrap() {
            ResolvedStorageLimits::Unlimited => false,
            ResolvedStorageLimits::Finite(limits) => {
                allocated_bytes >= limits.allocated_bytes
                    || filesystem_objects >= limits.filesystem_objects
                    || error.io_kind() == Some(std::io::ErrorKind::QuotaExceeded)
            }
        },
        Err(error) if error.is_terminal_failure() => {
            return EffectDecision::Invalidate;
        }
        Err(_) => return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo),
    };
    if quota_exhausted {
        return EffectDecision::ReturnFailure(FailureCause::AgentQuota);
    }

    let retry = match evidence {
        EffectEvidence::NoEffect if budget.available() => EffectDecision::RetryAfterProvenNoEffect,
        EffectEvidence::CompletedPrefix(_) if budget.available() => {
            EffectDecision::RetryUnwrittenSuffix
        }
        EffectEvidence::NoEffect | EffectEvidence::CompletedPrefix(_) => {
            return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo);
        }
        EffectEvidence::DesiredPostconditionSatisfied => return EffectDecision::Succeed,
        EffectEvidence::Unknown { .. } => return EffectDecision::Invalidate,
    };
    let Some(recovery) = &generation.pressure_recovery else {
        return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo);
    };
    let deadline = std::time::Instant::now() + WRITE_PRESSURE_RECOVERY_TIMEOUT;
    match recovery.recover_write(deadline).await {
        FilesystemWriteRecoveryOutcome::Recovered => retry,
        FilesystemWriteRecoveryOutcome::Denied => {
            EffectDecision::ReturnFailure(FailureCause::PhysicalCapacity)
        }
        FilesystemWriteRecoveryOutcome::NotUnderPressure
        | FilesystemWriteRecoveryOutcome::Unavailable => {
            EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo)
        }
    }
}

pub(super) async fn decide_resize_effect<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    error: &FilesystemStorageError,
    evidence: EffectEvidence,
    budget: RetryBudget,
    growing: bool,
) -> EffectDecision {
    if growing {
        decide_write_effect(generation, error, evidence, budget).await
    } else {
        decide_generation_effect(generation, error, evidence, budget).await
    }
}

/// Gives the error of a failure with `cause`. Each storage refusal goes through here, so the metric
/// counts each refused call once.
pub(super) fn classified_error(cause: FailureCause, source: FilesystemStorageError) -> Error {
    match cause {
        FailureCause::AgentQuota => {
            agent_filesystem::record_storage_refusal(StorageRefusal::AgentQuota);
            Error::AgentQuota(source)
        }
        FailureCause::PhysicalCapacity => {
            agent_filesystem::record_storage_refusal(StorageRefusal::PhysicalCapacity);
            Error::PhysicalCapacity(source)
        }
        FailureCause::TerminalInfrastructure => Error::RuntimeInvalidated,
        FailureCause::Guest | FailureCause::TransientBackend | FailureCause::UnclassifiedIo => {
            Error::Sandbox(source)
        }
    }
}

impl RetryBudget {
    pub(super) fn new(remaining: u8) -> Self {
        Self { remaining }
    }

    pub(super) fn available(self) -> bool {
        self.remaining != 0
    }

    pub(super) fn consume(&mut self) -> bool {
        if self.remaining == 0 {
            false
        } else {
            self.remaining -= 1;
            true
        }
    }
}

pub(super) fn decide_effect(
    cause: FailureCause,
    evidence: EffectEvidence,
    budget: RetryBudget,
) -> EffectDecision {
    match evidence {
        EffectEvidence::DesiredPostconditionSatisfied => EffectDecision::Succeed,
        EffectEvidence::Unknown { .. } => EffectDecision::Invalidate,
        _ if cause == FailureCause::TerminalInfrastructure => EffectDecision::Invalidate,
        EffectEvidence::CompletedPrefix(_)
            if budget.available()
                && matches!(
                    cause,
                    FailureCause::TransientBackend | FailureCause::UnclassifiedIo
                ) =>
        {
            EffectDecision::RetryUnwrittenSuffix
        }
        EffectEvidence::CompletedPrefix(_) => EffectDecision::ReturnFailure(cause),
        EffectEvidence::NoEffect => match cause {
            FailureCause::PhysicalCapacity if budget.available() => {
                EffectDecision::ReclaimCapacityThenRetry
            }
            FailureCause::TransientBackend | FailureCause::UnclassifiedIo if budget.available() => {
                EffectDecision::RetryAfterProvenNoEffect
            }
            FailureCause::TransientBackend | FailureCause::UnclassifiedIo => {
                EffectDecision::ReturnFailure(cause)
            }
            FailureCause::TerminalInfrastructure => EffectDecision::Invalidate,
            FailureCause::Guest | FailureCause::AgentQuota | FailureCause::PhysicalCapacity => {
                EffectDecision::ReturnFailure(cause)
            }
        },
    }
}

pub(super) fn classify_failure(
    error: &FilesystemStorageError,
    facts: FailureFacts,
) -> FailureCause {
    if invalidates_generation(error) {
        FailureCause::TerminalInfrastructure
    } else if error.is_storage_exhaustion() && facts.quota_exhausted {
        FailureCause::AgentQuota
    } else if error.is_storage_exhaustion() && facts.physical_capacity_exhausted {
        FailureCause::PhysicalCapacity
    } else {
        match error.io_kind() {
            Some(std::io::ErrorKind::NotFound)
            | Some(std::io::ErrorKind::AlreadyExists)
            | Some(std::io::ErrorKind::InvalidInput)
            | Some(std::io::ErrorKind::InvalidFilename)
            | Some(std::io::ErrorKind::IsADirectory)
            | Some(std::io::ErrorKind::NotADirectory)
            | Some(std::io::ErrorKind::CrossesDevices)
            | Some(std::io::ErrorKind::PermissionDenied) => FailureCause::Guest,
            Some(std::io::ErrorKind::WouldBlock) | Some(std::io::ErrorKind::Interrupted) => {
                FailureCause::TransientBackend
            }
            _ => FailureCause::UnclassifiedIo,
        }
    }
}

/// Tells whether a storage error in the tree of the agent invalidates the generation.
///
/// A sandbox escape refuses only the operation. Host storage failures, including permission
/// failures without a verified guest-policy refusal, invalidate the generation.
pub(super) fn invalidates_generation(error: &FilesystemStorageError) -> bool {
    error.is_terminal_failure()
}

pub(super) fn error_proves_no_effect(error: &FilesystemStorageError) -> bool {
    matches!(
        error.io_kind(),
        Some(std::io::ErrorKind::WouldBlock)
            | Some(std::io::ErrorKind::NotFound)
            | Some(std::io::ErrorKind::AlreadyExists)
            | Some(std::io::ErrorKind::InvalidInput)
            | Some(std::io::ErrorKind::InvalidFilename)
            | Some(std::io::ErrorKind::IsADirectory)
            | Some(std::io::ErrorKind::NotADirectory)
            | Some(std::io::ErrorKind::CrossesDevices)
            | Some(std::io::ErrorKind::PermissionDenied)
            | Some(std::io::ErrorKind::StorageFull)
            | Some(std::io::ErrorKind::QuotaExceeded)
    )
}
