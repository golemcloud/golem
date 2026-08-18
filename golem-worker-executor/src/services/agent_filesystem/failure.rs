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

pub(crate) type AgentFilesystemInvalidationCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
pub(crate) type AgentFilesystemRetryCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationEffect {
    ProvenNoEffect,
    KnownCompletedPrefix { bytes: u64 },
    DesiredPostconditionSatisfied,
    Unknown,
}

pub(crate) fn proven_write_progress_effect(completed: usize) -> MutationEffect {
    match u64::try_from(completed) {
        Ok(0) => MutationEffect::ProvenNoEffect,
        Ok(bytes) => MutationEffect::KnownCompletedPrefix { bytes },
        Err(_) => MutationEffect::Unknown,
    }
}

pub(crate) fn native_write_failure_effect(
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
pub(crate) enum MutationOperation {
    Write,
    Resize,
    Create,
    Metadata,
}

#[derive(Debug)]
pub(crate) enum MutationFailure<G> {
    Guest(G),
    StorageExhaustion { guest: G, quota_hint: bool },
    TransientGuest(G),
    AccessGuest(G),
    UnclassifiedGuest(G),
    Io(std::io::Error),
    Infrastructure(std::io::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationDecision<G> {
    PreserveGuest(G),
    Quota,
    InsufficientSpace,
    BoundedRetry,
    PreserveRaw,
    Success,
    Invalidate,
}

impl AgentFilesystemRuntime {
    pub(crate) async fn classify_mutation_failure<G>(
        &self,
        failure: MutationFailure<G>,
        effect: MutationEffect,
    ) -> MutationDecision<G> {
        self.classify_mutation_failure_for(MutationOperation::Metadata, failure, effect)
            .await
    }

    pub(crate) async fn classify_mutation_failure_for<G>(
        &self,
        operation: MutationOperation,
        failure: MutationFailure<G>,
        effect: MutationEffect,
    ) -> MutationDecision<G> {
        if effect == MutationEffect::Unknown {
            return self.invalidate().await;
        }
        if matches!(&failure, MutationFailure::Infrastructure(_))
            || matches!(&failure, MutationFailure::Io(error) if is_terminal_io(error))
        {
            return self.invalidate().await;
        }
        if effect == MutationEffect::DesiredPostconditionSatisfied {
            return MutationDecision::Success;
        }
        if let MutationFailure::Guest(error) = failure {
            return MutationDecision::PreserveGuest(error);
        }

        match failure {
            MutationFailure::Guest(_) => {
                unreachable!("guest failures return before classification")
            }
            MutationFailure::StorageExhaustion { guest, quota_hint } => {
                match self.fresh_failure_observations().await {
                    Ok((usage, limits, capacity)) => {
                        let quota_exhausted = usage.zip(limits).is_some_and(|(usage, limits)| {
                            quota_exhausted(operation, usage, limits)
                        });
                        let physical_exhausted = physical_exhausted(operation, capacity);
                        if quota_exhausted {
                            MutationDecision::Quota
                        } else if physical_exhausted {
                            MutationDecision::InsufficientSpace
                        } else {
                            tracing::warn!(
                                operation = ?operation,
                                quota_hint,
                                "Filesystem storage exhaustion was not explained by fresh quota or capacity observations"
                            );
                            MutationDecision::PreserveGuest(guest)
                        }
                    }
                    Err(error) if error.is_terminal_failure() => self.invalidate().await,
                    Err(_) if quota_hint => MutationDecision::PreserveGuest(guest),
                    Err(_) => MutationDecision::PreserveGuest(guest),
                }
            }
            MutationFailure::TransientGuest(guest) => {
                match self.fresh_failure_observations().await {
                    Ok(_) if self.retry_permitted().await => MutationDecision::BoundedRetry,
                    Ok(_) => MutationDecision::PreserveGuest(guest),
                    Err(error) if error.is_terminal_failure() => self.invalidate().await,
                    Err(_) => MutationDecision::PreserveGuest(guest),
                }
            }
            MutationFailure::AccessGuest(guest) => match self.fresh_failure_observations().await {
                Ok(_) => MutationDecision::PreserveGuest(guest),
                Err(_) => self.invalidate().await,
            },
            MutationFailure::UnclassifiedGuest(guest) => {
                match self.fresh_failure_observations().await {
                    Ok(_) if self.retry_permitted().await => MutationDecision::BoundedRetry,
                    Ok(_) => MutationDecision::PreserveGuest(guest),
                    Err(_) => self.invalidate().await,
                }
            }
            MutationFailure::Infrastructure(_) => self.invalidate().await,
            MutationFailure::Io(error) if is_terminal_io(&error) => self.invalidate().await,
            MutationFailure::Io(error) if is_storage_exhaustion(&error) => {
                match self.fresh_failure_observations().await {
                    Ok((usage, limits, capacity)) => {
                        let quota_exhausted = usage.zip(limits).is_some_and(|(usage, limits)| {
                            quota_exhausted(operation, usage, limits)
                        });
                        let physical_exhausted = physical_exhausted(operation, capacity);
                        if quota_exhausted {
                            MutationDecision::Quota
                        } else if physical_exhausted {
                            MutationDecision::InsufficientSpace
                        } else if is_quota_error(&error) {
                            tracing::warn!(
                                operation = ?operation,
                                raw_os_error = ?error.raw_os_error(),
                                "Filesystem storage exhaustion was not explained by fresh quota or capacity observations"
                            );
                            MutationDecision::Quota
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
            }
            MutationFailure::Io(error) if is_transient_io(&error) => {
                match self.fresh_failure_observations().await {
                    Ok(_) if self.retry_permitted().await => MutationDecision::BoundedRetry,
                    Ok(_) => MutationDecision::PreserveRaw,
                    Err(observation_error) if observation_error.is_terminal_failure() => {
                        self.invalidate().await
                    }
                    Err(_) => MutationDecision::PreserveRaw,
                }
            }
            MutationFailure::Io(error) if is_guest_scoped_io(&error) => {
                match self.fresh_failure_observations().await {
                    Ok(_) => MutationDecision::PreserveRaw,
                    Err(_) => self.invalidate().await,
                }
            }
            MutationFailure::Io(_) => match self.fresh_failure_observations().await {
                Ok(_) if self.retry_permitted().await => MutationDecision::BoundedRetry,
                Ok(_) => MutationDecision::PreserveRaw,
                Err(_) => self.invalidate().await,
            },
        }
    }

    pub(crate) fn set_invalidation_callback(
        &self,
        callback: Option<AgentFilesystemInvalidationCallback>,
    ) {
        *self
            .inner
            .invalidated
            .lock()
            .expect("agent filesystem invalidation callback lock poisoned") = callback;
    }

    pub(crate) fn set_retry_callback(&self, callback: Option<AgentFilesystemRetryCallback>) {
        *self
            .inner
            .retry_permitted
            .lock()
            .expect("agent filesystem retry callback lock poisoned") = callback;
    }

    async fn retry_permitted(&self) -> bool {
        if self.inner.state.load(Ordering::Acquire) & super::mutation::FILESYSTEM_RUNTIME_SEALED
            != 0
        {
            return false;
        }
        let callback = self
            .inner
            .retry_permitted
            .lock()
            .expect("agent filesystem retry callback lock poisoned")
            .clone();
        match callback {
            Some(callback) => callback().await,
            None => true,
        }
    }

    async fn invalidate<G>(&self) -> MutationDecision<G> {
        self.seal();
        if !self
            .inner
            .invalidation_notified
            .swap(true, Ordering::AcqRel)
        {
            let callback = self
                .inner
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
        let _: MutationDecision<()> = self.invalidate().await;
    }
}

fn quota_exhausted(
    operation: MutationOperation,
    usage: AgentFilesystemUsage,
    limits: ResolvedAgentFilesystemLimits,
) -> bool {
    let bytes_exhausted = usage.allocated_bytes >= limits.allocated_bytes;
    let objects_exhausted = usage.filesystem_objects >= limits.filesystem_objects;
    match operation {
        MutationOperation::Write | MutationOperation::Resize | MutationOperation::Metadata => {
            bytes_exhausted
        }
        MutationOperation::Create => bytes_exhausted || objects_exhausted,
    }
}

fn physical_exhausted(operation: MutationOperation, capacity: FilesystemCapacity) -> bool {
    let bytes_exhausted = capacity.available_bytes == 0;
    let objects_exhausted = capacity.available_filesystem_objects == 0;
    match operation {
        MutationOperation::Write | MutationOperation::Resize | MutationOperation::Metadata => {
            bytes_exhausted
        }
        MutationOperation::Create => bytes_exhausted || objects_exhausted,
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
