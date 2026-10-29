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

use crate::metrics::agent_filesystem::{self, PressureRecovery, PressureUnload};
use crate::services::active_agents::{
    ActiveAgents, FilesystemPressureVictim, eligible_loaded_idle_filesystem_pressure_victims,
    request_loaded_idle_filesystem_unload,
};
use crate::services::golem_config::FilesystemPressureConfig;
use crate::storage_volume::StorageVolumeError;
use crate::storage_volume::{FilesystemSpace, FilesystemVolume, observe_space};
use crate::worker::{EvictionStopOutcome, UnloadReason, UnloadRequest};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

static RECOVERY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl FilesystemPressureConfig {
    pub(crate) fn validate_capacity(
        &self,
        total_bytes: u64,
        total_filesystem_objects: u64,
    ) -> Result<(), StorageVolumeError> {
        if self.target_available_bytes() > total_bytes {
            return Err(StorageVolumeError::verification_without_path(
                "fit filesystem pressure byte target within the volume capacity",
            ));
        }
        if self.target_available_filesystem_objects() > total_filesystem_objects {
            return Err(StorageVolumeError::verification_without_path(
                "fit filesystem pressure object target within the volume capacity",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemCapacityTarget {
    available_bytes: Option<u64>,
    available_filesystem_objects: Option<u64>,
    reclamation_observation_attempts: u32,
    reclamation_observation_delay: Duration,
}

impl FilesystemCapacityTarget {
    #[cfg(test)]
    pub(crate) fn available_bytes(
        available_bytes: u64,
        reclamation_observation_attempts: u32,
        reclamation_observation_delay: Duration,
    ) -> Self {
        Self {
            available_bytes: Some(available_bytes),
            available_filesystem_objects: None,
            reclamation_observation_attempts,
            reclamation_observation_delay,
        }
    }

    #[cfg(test)]
    pub(crate) fn available_filesystem_objects(
        available_filesystem_objects: u64,
        reclamation_observation_attempts: u32,
        reclamation_observation_delay: Duration,
    ) -> Self {
        Self {
            available_bytes: None,
            available_filesystem_objects: Some(available_filesystem_objects),
            reclamation_observation_attempts,
            reclamation_observation_delay,
        }
    }

    pub(crate) fn combined(
        available_bytes: u64,
        available_filesystem_objects: u64,
        reclamation_observation_attempts: u32,
        reclamation_observation_delay: Duration,
    ) -> Self {
        Self {
            available_bytes: Some(available_bytes),
            available_filesystem_objects: Some(available_filesystem_objects),
            reclamation_observation_attempts,
            reclamation_observation_delay,
        }
    }

    fn reached(self, observation: FilesystemSpace) -> bool {
        match observation {
            FilesystemSpace::Unlimited => true,
            FilesystemSpace::Observed {
                available_bytes,
                available_filesystem_objects,
                ..
            } => {
                self.available_bytes
                    .is_none_or(|target| available_bytes >= target)
                    && self
                        .available_filesystem_objects
                        .is_none_or(|target| available_filesystem_objects >= target)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CapacityReclamation {
    pub(crate) final_observation: FilesystemSpace,
    pub(crate) unloaded_agents: u32,
    pub(crate) target_reached: bool,
}

impl CapacityReclamation {
    pub(crate) fn permits_pressure_retry(self) -> bool {
        self.target_reached && matches!(self.final_observation, FilesystemSpace::Observed { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemWritePressurePolicy {
    minimum_available_bytes: u64,
    minimum_available_filesystem_objects: u64,
    target: FilesystemCapacityTarget,
}

impl FilesystemWritePressurePolicy {
    pub(crate) fn from_config(config: &FilesystemPressureConfig) -> Self {
        Self {
            minimum_available_bytes: config.minimum_available_bytes(),
            minimum_available_filesystem_objects: config.minimum_available_filesystem_objects(),
            target: FilesystemCapacityTarget::combined(
                config.target_available_bytes(),
                config.target_available_filesystem_objects(),
                config.reclamation_observation_attempts(),
                config.reclamation_observation_delay(),
            ),
        }
    }

    fn physical_pressure(self, observation: FilesystemSpace) -> bool {
        matches!(
            observation,
            FilesystemSpace::Observed {
                available_bytes,
                available_filesystem_objects,
                ..
            } if available_bytes <= self.minimum_available_bytes
                || available_filesystem_objects <= self.minimum_available_filesystem_objects
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilesystemWriteRecoveryOutcome {
    NotUnderPressure,
    Unavailable,
    Recovered,
    Denied,
}

#[async_trait]
pub(crate) trait FilesystemWriteRecoveryAuthority: Send + Sync + 'static {
    async fn recover_write(&self, deadline: Instant) -> FilesystemWriteRecoveryOutcome;
}

#[derive(Clone)]
pub(crate) struct FilesystemWriteRecovery {
    authority: Arc<dyn FilesystemWriteRecoveryAuthority>,
}

impl FilesystemWriteRecovery {
    pub(crate) fn for_active_agents<Ctx: WorkerCtx>(
        volume: FilesystemVolume,
        active_agents: Weak<ActiveAgents<Ctx>>,
        policy: FilesystemWritePressurePolicy,
    ) -> Self {
        Self {
            authority: Arc::new(ProductionWriteRecovery {
                volume,
                active_agents,
                policy,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted(authority: Arc<dyn FilesystemWriteRecoveryAuthority>) -> Self {
        Self { authority }
    }

    pub(crate) async fn recover_write(&self, deadline: Instant) -> FilesystemWriteRecoveryOutcome {
        self.authority.recover_write(deadline).await
    }
}

#[derive(Debug)]
pub(crate) enum FilesystemPressureError {
    Deadline,
    Observation(StorageVolumeError),
    CleanupFailed,
}

impl Display for FilesystemPressureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deadline => {
                formatter.write_str("filesystem capacity reclamation deadline expired")
            }
            Self::Observation(error) => Display::fmt(error, formatter),
            Self::CleanupFailed => formatter.write_str("filesystem pressure victim cleanup failed"),
        }
    }
}

impl std::error::Error for FilesystemPressureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Observation(error) => Some(error),
            Self::Deadline | Self::CleanupFailed => None,
        }
    }
}

#[async_trait]
trait CapacityRecoverySource: Sync {
    type Candidate: Send;

    async fn observe_space(&self) -> Result<FilesystemSpace, FilesystemPressureError>;
    async fn eligible_loaded_idle_victims(&self) -> Vec<Self::Candidate>;
    async fn request_unload(
        &self,
        candidate: Self::Candidate,
        deadline: Instant,
    ) -> Result<EvictionStopOutcome, FilesystemPressureError>;
}

struct ProductionRecovery<'a, Ctx: WorkerCtx> {
    volume: &'a FilesystemVolume,
    active_agents: &'a ActiveAgents<Ctx>,
}

struct ProductionWriteRecovery<Ctx: WorkerCtx> {
    volume: FilesystemVolume,
    active_agents: Weak<ActiveAgents<Ctx>>,
    policy: FilesystemWritePressurePolicy,
}

#[async_trait]
impl<Ctx: WorkerCtx> FilesystemWriteRecoveryAuthority for ProductionWriteRecovery<Ctx> {
    async fn recover_write(&self, deadline: Instant) -> FilesystemWriteRecoveryOutcome {
        let Some(active_agents) = self.active_agents.upgrade() else {
            agent_filesystem::record_pressure_recovery(
                PressureRecovery::Unavailable,
                Duration::ZERO,
            );
            return FilesystemWriteRecoveryOutcome::Unavailable;
        };
        recover_write_from(
            &ProductionRecovery {
                volume: &self.volume,
                active_agents: active_agents.as_ref(),
            },
            &RECOVERY,
            self.policy,
            deadline,
        )
        .await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> CapacityRecoverySource for ProductionRecovery<'_, Ctx> {
    type Candidate = FilesystemPressureVictim<Ctx>;

    async fn observe_space(&self) -> Result<FilesystemSpace, FilesystemPressureError> {
        observe_space(self.volume)
            .await
            .map_err(FilesystemPressureError::Observation)
    }

    async fn eligible_loaded_idle_victims(&self) -> Vec<Self::Candidate> {
        let candidates = eligible_loaded_idle_filesystem_pressure_victims(self.active_agents).await;
        let mut ordered = candidates
            .into_iter()
            .map(|candidate| {
                (
                    candidate.eligible_since(),
                    candidate.stable_agent_id().to_owned(),
                    candidate,
                )
            })
            .collect::<Vec<_>>();
        sort_filesystem_pressure_candidates(&mut ordered);
        ordered
            .into_iter()
            .map(|(_, _, candidate)| candidate)
            .collect()
    }

    async fn request_unload(
        &self,
        candidate: Self::Candidate,
        deadline: Instant,
    ) -> Result<EvictionStopOutcome, FilesystemPressureError> {
        request_loaded_idle_filesystem_unload(
            candidate,
            filesystem_pressure_unload_request(deadline),
        )
        .await
        .map_err(|error| {
            tracing::warn!(error = %error, "Filesystem pressure unload task failed");
            FilesystemPressureError::CleanupFailed
        })
    }
}

/// Recovers free space for a write, and records the time and the outcome in the metric.
async fn recover_write_from<S: CapacityRecoverySource>(
    source: &S,
    recovery: &tokio::sync::Mutex<()>,
    policy: FilesystemWritePressurePolicy,
    deadline: Instant,
) -> FilesystemWriteRecoveryOutcome {
    let started = Instant::now();
    let (recorded, outcome) = recover_write_classified(source, recovery, policy, deadline).await;
    agent_filesystem::record_pressure_recovery(recorded, started.elapsed());
    outcome
}

/// Recovers free space for a write. Gives the outcome for the metric, which tells the causes of
/// `Unavailable` and `Denied` apart, and the outcome for the write.
async fn recover_write_classified<S: CapacityRecoverySource>(
    source: &S,
    recovery: &tokio::sync::Mutex<()>,
    policy: FilesystemWritePressurePolicy,
    deadline: Instant,
) -> (PressureRecovery, FilesystemWriteRecoveryOutcome) {
    let observation = match before_deadline(deadline, source.observe_space()).await {
        Ok(Ok(observation)) => observation,
        Ok(Err(_)) => {
            return (
                PressureRecovery::ObservationFailed,
                FilesystemWriteRecoveryOutcome::Unavailable,
            );
        }
        Err(_) => {
            return (
                PressureRecovery::Deadline,
                FilesystemWriteRecoveryOutcome::Unavailable,
            );
        }
    };
    if !policy.physical_pressure(observation) {
        return (
            PressureRecovery::NotUnderPressure,
            FilesystemWriteRecoveryOutcome::NotUnderPressure,
        );
    }

    let recorded =
        match reclaim_filesystem_capacity_from(source, recovery, policy.target, deadline).await {
            Ok(reclamation) if reclamation.permits_pressure_retry() => {
                return (
                    PressureRecovery::Recovered,
                    FilesystemWriteRecoveryOutcome::Recovered,
                );
            }
            Ok(_) => PressureRecovery::TargetNotReached,
            Err(FilesystemPressureError::Deadline) => PressureRecovery::Deadline,
            Err(FilesystemPressureError::Observation(_)) => PressureRecovery::ObservationFailed,
            Err(FilesystemPressureError::CleanupFailed) => PressureRecovery::CleanupFailed,
        };
    (recorded, FilesystemWriteRecoveryOutcome::Denied)
}

async fn reclaim_filesystem_capacity_from<S: CapacityRecoverySource>(
    source: &S,
    recovery: &tokio::sync::Mutex<()>,
    target: FilesystemCapacityTarget,
    deadline: Instant,
) -> Result<CapacityReclamation, FilesystemPressureError> {
    let initial = before_deadline(deadline, source.observe_space()).await??;
    if target.reached(initial) {
        return Ok(CapacityReclamation {
            final_observation: initial,
            unloaded_agents: 0,
            target_reached: true,
        });
    }
    if Instant::now() >= deadline {
        return Err(FilesystemPressureError::Deadline);
    }

    let _recovery = before_deadline(deadline, recovery.lock()).await?;
    let mut final_observation = before_deadline(deadline, source.observe_space()).await??;
    if target.reached(final_observation) {
        return Ok(CapacityReclamation {
            final_observation,
            unloaded_agents: 0,
            target_reached: true,
        });
    }

    let candidates = before_deadline(deadline, source.eligible_loaded_idle_victims()).await?;
    let mut unloaded_agents = 0u32;
    for candidate in candidates {
        let unload = before_deadline(deadline, source.request_unload(candidate, deadline)).await;
        agent_filesystem::record_pressure_unload_attempt(match &unload {
            Ok(Ok(EvictionStopOutcome::Unloaded)) => PressureUnload::Unloaded,
            Ok(Ok(EvictionStopOutcome::Ineligible)) => PressureUnload::Ineligible,
            Ok(Ok(EvictionStopOutcome::CleanupFailed)) => PressureUnload::CleanupFailed,
            Ok(Err(_)) => PressureUnload::TaskFailed,
            Err(_) => PressureUnload::Deadline,
        });
        match unload?? {
            EvictionStopOutcome::Ineligible => continue,
            EvictionStopOutcome::CleanupFailed => {
                return Err(FilesystemPressureError::CleanupFailed);
            }
            EvictionStopOutcome::Unloaded => {
                crate::metrics::workers::record_worker_eviction("FilesystemPressureLoadedIdle");
                unloaded_agents = unloaded_agents.saturating_add(1);
            }
        }

        for observation_attempt in 0..target.reclamation_observation_attempts {
            final_observation = before_deadline(deadline, source.observe_space()).await??;
            if target.reached(final_observation) {
                return Ok(CapacityReclamation {
                    final_observation,
                    unloaded_agents,
                    target_reached: true,
                });
            }
            if observation_attempt + 1 < target.reclamation_observation_attempts {
                before_deadline(
                    deadline,
                    tokio::time::sleep(target.reclamation_observation_delay),
                )
                .await?;
            }
        }
    }

    Ok(CapacityReclamation {
        final_observation,
        unloaded_agents,
        target_reached: false,
    })
}

fn filesystem_pressure_unload_request(deadline: Instant) -> UnloadRequest {
    UnloadRequest::new(UnloadReason::FilesystemPressure, deadline)
}

fn sort_filesystem_pressure_candidates<T>(candidates: &mut [(u64, String, T)]) {
    candidates.sort_by(|left, right| (left.0, left.1.as_str()).cmp(&(right.0, right.1.as_str())));
}

async fn before_deadline<T>(
    deadline: Instant,
    future: impl Future<Output = T>,
) -> Result<T, FilesystemPressureError> {
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future)
        .await
        .map_err(|_| FilesystemPressureError::Deadline)
}

#[cfg(test)]
mod tests;
