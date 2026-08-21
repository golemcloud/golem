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
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;

pub(super) type AgentFilesystemInvalidationCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
pub(super) type AgentFilesystemRetryCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;
pub(super) type AgentFilesystemPressureRecoveryCallback = Arc<
    dyn Fn(
            FilesystemPressureOperation,
            std::time::Instant,
        ) -> Pin<Box<dyn Future<Output = bool> + Send>>
        + Send
        + Sync,
>;

pub(super) const FILESYSTEM_MUTATION_MAX_ATTEMPTS: usize = 2;
pub(super) const FILESYSTEM_MUTATION_RETRY_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MutationEffect {
    ProvenNoEffect,
    KnownCompletedPrefix { bytes: u64 },
    DesiredPostconditionSatisfied,
    Unknown,
}

pub(super) fn proven_write_progress_effect(completed: usize) -> MutationEffect {
    match u64::try_from(completed) {
        Ok(0) => MutationEffect::ProvenNoEffect,
        Ok(bytes) => MutationEffect::KnownCompletedPrefix { bytes },
        Err(_) => MutationEffect::Unknown,
    }
}

pub(super) fn native_write_failure_effect(
    error: &std::io::Error,
    completed: usize,
) -> MutationEffect {
    if write_error_proves_no_effect(error) {
        proven_write_progress_effect(completed)
    } else {
        MutationEffect::Unknown
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilesystemPressureOperation {
    Write,
    Resize,
    Create,
    Metadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MutationDecision {
    Quota,
    InsufficientSpace,
    PhysicalPressure,
    BoundedRetry,
    PreserveRaw,
    Success,
    Invalidate,
}

impl AgentFilesystemRuntime {
    pub(super) async fn classify_io_failure(
        &self,
        operation: FilesystemPressureOperation,
        error: std::io::Error,
        effect: MutationEffect,
    ) -> MutationDecision {
        if effect != MutationEffect::ProvenNoEffect
            && self.observe_usage_for_billing().await.is_err()
        {
            return self.invalidate().await;
        }
        if effect == MutationEffect::Unknown {
            return self.invalidate().await;
        }
        if is_terminal_io(&error) {
            return self.invalidate().await;
        }
        if effect == MutationEffect::DesiredPostconditionSatisfied {
            return MutationDecision::Success;
        }
        if is_storage_exhaustion(&error) {
            match self.fresh_failure_observations().await {
                Ok((usage, limits, capacity)) => {
                    let quota_exhausted = usage
                        .zip(limits)
                        .is_some_and(|(usage, limits)| quota_exhausted(operation, usage, limits));
                    let physical_exhausted = self
                        .runtime_state
                        .pressure
                        .pressure(operation, capacity)
                        .is_some();
                    if is_quota_error(&error) || quota_exhausted {
                        MutationDecision::Quota
                    } else if physical_exhausted {
                        MutationDecision::PhysicalPressure
                    } else {
                        tracing::warn!(
                            operation = ?operation,
                            raw_os_error = ?error.raw_os_error(),
                            "Filesystem storage exhaustion was not explained by fresh quota or capacity observations"
                        );
                        MutationDecision::InsufficientSpace
                    }
                }
                Err(observation_error) if observation_error.is_terminal_failure() => {
                    self.invalidate().await
                }
                Err(_) if is_quota_error(&error) => MutationDecision::Quota,
                Err(_) => MutationDecision::InsufficientSpace,
            }
        } else if is_transient_io(&error) {
            match self.fresh_failure_observations().await {
                Ok(_) if self.retry_permitted().await => MutationDecision::BoundedRetry,
                Ok(_) => MutationDecision::PreserveRaw,
                Err(observation_error) if observation_error.is_terminal_failure() => {
                    self.invalidate().await
                }
                Err(_) => MutationDecision::PreserveRaw,
            }
        } else if is_guest_scoped_io(&error) {
            match self.fresh_failure_observations().await {
                Ok(_) => MutationDecision::PreserveRaw,
                Err(_) => self.invalidate().await,
            }
        } else {
            match self.fresh_failure_observations().await {
                Ok(_) if self.retry_permitted().await => MutationDecision::BoundedRetry,
                Ok(_) => MutationDecision::PreserveRaw,
                Err(_) => self.invalidate().await,
            }
        }
    }

    pub(crate) fn set_invalidation_callback(
        &self,
        callback: Option<AgentFilesystemInvalidationCallback>,
    ) {
        *self
            .runtime_state
            .invalidated
            .lock()
            .expect("agent filesystem invalidation callback lock poisoned") = callback;
    }

    pub(crate) fn set_retry_callback(&self, callback: Option<AgentFilesystemRetryCallback>) {
        *self
            .runtime_state
            .retry_permitted
            .lock()
            .expect("agent filesystem retry callback lock poisoned") = callback;
    }

    pub(crate) fn set_pressure_recovery_callback(
        &self,
        callback: Option<AgentFilesystemPressureRecoveryCallback>,
    ) {
        *self
            .runtime_state
            .pressure_recovery
            .lock()
            .expect("agent filesystem pressure callback lock poisoned") = callback;
    }

    pub(crate) async fn recover_physical_pressure(
        &self,
        operation: FilesystemPressureOperation,
        deadline: std::time::Instant,
    ) -> bool {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        if !self.retry_permitted().await {
            return false;
        }
        let callback = self
            .runtime_state
            .pressure_recovery
            .lock()
            .expect("agent filesystem pressure callback lock poisoned")
            .clone();
        match callback {
            Some(callback) if std::time::Instant::now() < deadline => tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                callback(operation, deadline),
            )
            .await
            .unwrap_or(false),
            Some(_) | None => false,
        }
    }

    async fn retry_permitted(&self) -> bool {
        if self.runtime_state.is_sealed() {
            return false;
        }
        let callback = self
            .runtime_state
            .retry_permitted
            .lock()
            .expect("agent filesystem retry callback lock poisoned")
            .clone();
        match callback {
            Some(callback) => callback().await,
            None => true,
        }
    }

    async fn invalidate(&self) -> MutationDecision {
        self.seal();
        if !self
            .runtime_state
            .invalidation_notified
            .swap(true, Ordering::AcqRel)
        {
            let callback = self
                .runtime_state
                .invalidated
                .lock()
                .expect("agent filesystem invalidation callback lock poisoned")
                .clone();
            if let Some(callback) = callback {
                callback().await;
            }
        }
        MutationDecision::Invalidate
    }

    pub(crate) async fn invalidate_runtime(&self) {
        let _ = self.invalidate().await;
    }
}

fn quota_exhausted(
    operation: FilesystemPressureOperation,
    usage: AgentFilesystemUsage,
    limits: ResolvedAgentFilesystemLimits,
) -> bool {
    let bytes_exhausted = usage.allocated_bytes >= limits.allocated_bytes;
    let objects_exhausted = usage.filesystem_objects >= limits.filesystem_objects;
    match operation {
        FilesystemPressureOperation::Write
        | FilesystemPressureOperation::Resize
        | FilesystemPressureOperation::Metadata => bytes_exhausted,
        FilesystemPressureOperation::Create => bytes_exhausted || objects_exhausted,
    }
}

fn is_storage_exhaustion(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
    ) || is_enospc(error)
        || is_edquot(error)
}

fn is_quota_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::QuotaExceeded || is_edquot(error)
}

fn is_transient_io(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    ) || is_ebusy(error)
        || is_eagain(error)
}

fn write_error_proves_no_effect(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || is_ebusy(error)
        || is_storage_exhaustion(error)
        || is_guest_scoped_io(error)
}

fn is_guest_scoped_io(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::AlreadyExists
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::InvalidFilename
            | std::io::ErrorKind::IsADirectory
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::FileTooLarge
    )
}

fn is_terminal_io(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
    ) || is_eio(error)
        || is_estale(error)
        || is_enodev(error)
}

#[cfg(target_os = "linux")]
fn is_enospc(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOSPC)
}

#[cfg(not(target_os = "linux"))]
fn is_enospc(_error: &std::io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_edquot(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EDQUOT)
}

#[cfg(not(target_os = "linux"))]
fn is_edquot(_error: &std::io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_ebusy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EBUSY)
}

#[cfg(not(target_os = "linux"))]
fn is_ebusy(_error: &std::io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_eagain(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EAGAIN)
}

#[cfg(not(target_os = "linux"))]
fn is_eagain(_error: &std::io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_eio(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EIO)
}

#[cfg(not(target_os = "linux"))]
fn is_eio(_error: &std::io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_estale(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ESTALE)
}

#[cfg(not(target_os = "linux"))]
fn is_estale(_error: &std::io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_enodev(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENODEV)
}

#[cfg(not(target_os = "linux"))]
fn is_enodev(_error: &std::io::Error) -> bool {
    false
}
