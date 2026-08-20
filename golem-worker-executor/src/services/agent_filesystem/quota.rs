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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentFilesystemUsage {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentFilesystemStorageLimit {
    pub allocated_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedAgentFilesystemLimits {
    pub allocated_bytes: u64,
    pub filesystem_objects: u64,
    pub filesystem_object_limit_policy_version: u32,
}

pub(super) type FilesystemLimitExceededCallback =
    Arc<dyn Fn(bool) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub(super) const FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION: u32 = 2;
const BYTES_PER_GIB: u128 = 1024 * 1024 * 1024;

impl FilesystemObjectLimitPolicyConfig {
    pub(super) fn validate(&self) -> Result<(), FilesystemStorageError> {
        let valid = self.objects_per_gib != 0
            && self.minimum_objects != 0
            && self.maximum_objects != 0
            && self.minimum_objects <= self.maximum_objects;
        if valid {
            Ok(())
        } else {
            Err(FilesystemStorageError::verification(
                "validate filesystem object limit policy",
                Path::new("<configuration>"),
            ))
        }
    }

    pub(super) fn resolve(
        &self,
        limit: AgentFilesystemStorageLimit,
    ) -> Result<ResolvedAgentFilesystemLimits, FilesystemStorageError> {
        self.validate()?;
        if limit.allocated_bytes == 0 {
            return Err(FilesystemStorageError::verification(
                "resolve nonzero agent filesystem storage limit",
                Path::new("<configuration>"),
            ));
        }

        let proportional = (u128::from(limit.allocated_bytes) * u128::from(self.objects_per_gib))
            .div_ceil(BYTES_PER_GIB);
        let proportional = u64::try_from(proportional).map_err(|_| {
            FilesystemStorageError::verification(
                "derive agent filesystem object limit",
                Path::new("<configuration>"),
            )
        })?;

        Ok(ResolvedAgentFilesystemLimits {
            allocated_bytes: limit.allocated_bytes,
            filesystem_objects: proportional.clamp(self.minimum_objects, self.maximum_objects),
            filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "authoritative capacity observation is part of the filesystem interface"
)]
pub(crate) struct FilesystemCapacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub total_filesystem_objects: u64,
    pub available_filesystem_objects: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemPressure {
    bytes: bool,
    filesystem_objects: bool,
}

impl FilesystemPressure {
    pub(crate) fn include(self, pressure: Option<Self>) -> Self {
        match pressure {
            Some(pressure) => Self {
                bytes: self.bytes || pressure.bytes,
                filesystem_objects: self.filesystem_objects || pressure.filesystem_objects,
            },
            None => self,
        }
    }
}

impl FilesystemPressureConfig {
    pub(super) fn validate(&self) -> Result<(), FilesystemStorageError> {
        if self.minimum_available_bytes < self.target_available_bytes
            && self.minimum_available_filesystem_objects < self.target_available_filesystem_objects
            && self.reclamation_observation_attempts != 0
        {
            Ok(())
        } else {
            Err(FilesystemStorageError::verification(
                "validate filesystem pressure watermarks",
                Path::new("<configuration>"),
            ))
        }
    }

    pub(super) fn validate_capacity(
        &self,
        capacity: FilesystemCapacity,
    ) -> Result<(), FilesystemStorageError> {
        if self.target_available_bytes <= capacity.total_bytes {
            Ok(())
        } else {
            Err(FilesystemStorageError::verification(
                "fit filesystem pressure byte target within managed capacity",
                Path::new("<configuration>"),
            ))
        }
    }

    pub(crate) fn pressure(
        &self,
        operation: MutationOperation,
        capacity: FilesystemCapacity,
    ) -> Option<FilesystemPressure> {
        let bytes = capacity.available_bytes <= self.minimum_available_bytes;
        let filesystem_objects = operation == MutationOperation::Create
            && capacity.available_filesystem_objects <= self.minimum_available_filesystem_objects;
        (bytes || filesystem_objects).then_some(FilesystemPressure {
            bytes,
            filesystem_objects,
        })
    }

    pub(crate) fn target_reached(
        &self,
        pressure: FilesystemPressure,
        capacity: FilesystemCapacity,
    ) -> bool {
        (!pressure.bytes || capacity.available_bytes >= self.target_available_bytes)
            && (!pressure.filesystem_objects
                || capacity.available_filesystem_objects
                    >= self.target_available_filesystem_objects)
    }
}

impl AgentFilesystems {
    #[allow(
        dead_code,
        reason = "authoritative capacity observation is part of the filesystem interface"
    )]
    pub(crate) async fn observe_capacity(
        &self,
    ) -> Result<FilesystemCapacity, FilesystemStorageError> {
        self.provisioner.observe_capacity().await
    }

    pub(crate) fn pressure_policy(&self) -> &FilesystemPressureConfig {
        &self.pressure
    }
}

impl AgentFilesystem {
    #[allow(
        dead_code,
        reason = "authoritative usage observation is part of the filesystem interface"
    )]
    pub(crate) async fn usage(
        &self,
    ) -> Result<Option<AgentFilesystemUsage>, FilesystemStorageError> {
        self.runtime.usage().await
    }

    pub(crate) async fn settle_reconstruction(&self) -> Result<(), FilesystemStorageError> {
        let _effect = self.runtime.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "settle reconstructed agent filesystem",
                self.runtime.inner.backend.root(),
                std::io::Error::other(error),
            )
        })?;
        Ok(())
    }
}

impl AgentFilesystemRuntime {
    pub(crate) async fn usage(
        &self,
    ) -> Result<Option<AgentFilesystemUsage>, FilesystemStorageError> {
        #[cfg(test)]
        if self.inner.usage_observation_fails.load(Ordering::Acquire) {
            return Err(FilesystemStorageError::verification(
                "observe test agent filesystem usage",
                self.inner.backend.root(),
            ));
        }
        match self.inner.backend.quota() {
            Some(quota) => quota.usage().await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) fn set_usage_observer(
        &self,
        observer: Option<Arc<dyn crate::services::agent_resource_billing::FilesystemUsageObserver>>,
    ) {
        *self
            .inner
            .usage_observer
            .lock()
            .expect("agent filesystem usage-observer lock poisoned") = observer;
        if self.has_active_effects() {
            self.inner.schedule_usage_sampling();
        }
    }

    pub(super) fn usage_observer_is_active(&self) -> bool {
        self.inner
            .usage_observer
            .lock()
            .expect("agent filesystem usage-observer lock poisoned")
            .as_ref()
            .is_some_and(|observer| observer.is_active())
    }

    pub(super) async fn observe_usage_for_billing(&self) -> Result<(), FilesystemStorageError> {
        let observer = self
            .inner
            .usage_observer
            .lock()
            .expect("agent filesystem usage-observer lock poisoned")
            .clone();
        let Some(observer) = observer else {
            return Ok(());
        };
        let observation = observer.begin_observation();
        match self.usage().await {
            Ok(usage) => {
                observer.complete_observation(observation, usage, Instant::now());
                Ok(())
            }
            Err(error) => {
                if observer.fail_observation(observation) {
                    Err(error)
                } else {
                    Ok(())
                }
            }
        }
    }

    #[allow(
        dead_code,
        reason = "fresh physical capacity is part of the runtime filesystem interface"
    )]
    pub(crate) async fn observe_capacity(
        &self,
    ) -> Result<FilesystemCapacity, FilesystemStorageError> {
        self.inner.backend.observe_capacity().await
    }

    #[allow(
        dead_code,
        reason = "fresh failure observations support runtime mutation classification"
    )]
    pub(super) async fn fresh_failure_observations(
        &self,
    ) -> Result<
        (
            Option<AgentFilesystemUsage>,
            Option<ResolvedAgentFilesystemLimits>,
            FilesystemCapacity,
        ),
        FilesystemStorageError,
    > {
        let observer = self
            .inner
            .usage_observer
            .lock()
            .expect("agent filesystem usage-observer lock poisoned")
            .clone();
        let observation = observer
            .as_ref()
            .map(|observer| observer.begin_observation());

        #[cfg(test)]
        if let Some((usage, capacity)) = *self
            .inner
            .failure_observations
            .read()
            .expect("agent filesystem test observation lock poisoned")
        {
            let limits = *self
                .inner
                .applied_limits
                .read()
                .expect("agent filesystem applied-limit lock poisoned");
            if let (Some(observer), Some(observation)) = (observer, observation) {
                observer.complete_observation(observation, usage, Instant::now());
            }
            return Ok((usage, limits, capacity));
        }

        let installed_limits = *self
            .inner
            .applied_limits
            .read()
            .expect("agent filesystem applied-limit lock poisoned");
        let observations = async {
            match self.inner.backend.quota() {
                Some(quota) => quota
                    .failure_observations(installed_limits)
                    .await
                    .map(|(usage, limits)| (Some(usage), limits)),
                None => Ok((None, None)),
            }
        };
        let (observations, capacity) = tokio::join!(observations, self.observe_capacity());
        let (usage, limits) = match observations {
            Ok(observations) => observations,
            Err(error) => {
                if let (Some(observer), Some(observation)) = (&observer, observation) {
                    let _ = observer.fail_observation(observation);
                }
                return Err(error);
            }
        };
        if let (Some(observer), Some(observation)) = (&observer, observation) {
            observer.complete_observation(observation, usage, Instant::now());
        }
        let capacity = capacity?;
        Ok((usage, limits, capacity))
    }

    pub(super) fn set_limit_exceeded_callback(
        &self,
        callback: Option<FilesystemLimitExceededCallback>,
    ) {
        *self
            .inner
            .limit_exceeded
            .lock()
            .expect("agent filesystem limit callback lock poisoned") = callback;
    }

    pub(crate) fn is_same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) async fn set_allocated_byte_limit(
        &self,
        limit: AgentFilesystemStorageLimit,
    ) -> Result<(), FilesystemStorageError> {
        let effect = self.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "admit agent filesystem limit update",
                self.inner.backend.root(),
                std::io::Error::other(error),
            )
        })?;
        let Some(quota) = self.inner.backend.quota() else {
            return Ok(());
        };
        let installed = quota.install_limit(limit, effect.clone()).await?;
        *self
            .inner
            .applied_limits
            .write()
            .expect("agent filesystem applied-limit lock poisoned") = Some(installed.limits);
        let exceeded = installed.usage.allocated_bytes > installed.limits.allocated_bytes
            || installed.usage.filesystem_objects > installed.limits.filesystem_objects;
        self.notify_limit_state(exceeded).await;
        drop(effect);
        Ok(())
    }

    pub(super) async fn notify_limit_state(&self, exceeded: bool) {
        let callback = self
            .inner
            .limit_exceeded
            .lock()
            .expect("agent filesystem limit callback lock poisoned")
            .clone();
        if let Some(callback) = callback {
            callback(exceeded).await;
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn validate_observed_limits(
    root: &Path,
    installed: Option<ResolvedAgentFilesystemLimits>,
    observed: Option<ResolvedAgentFilesystemLimits>,
) -> Result<(), FilesystemStorageError> {
    if installed == observed {
        Ok(())
    } else {
        Err(FilesystemStorageError::io(
            "validate managed XFS project quota limits",
            root,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("installed limits {installed:?} differ from observed limits {observed:?}"),
            ),
        ))
    }
}

#[cfg(target_os = "linux")]
#[allow(
    dead_code,
    reason = "fresh physical capacity is part of the runtime filesystem interface"
)]
pub(super) async fn observe_path_capacity(
    root: &Path,
) -> Result<FilesystemCapacity, FilesystemStorageError> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking({
        let root = root.clone();
        move || {
            let capacity = rustix::fs::statvfs(&root)
                .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
            if capacity
                .f_flag
                .contains(rustix::fs::StatVfsMountFlags::RDONLY)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ReadOnlyFilesystem,
                    "agent filesystem mount is read-only",
                ));
            }
            #[cfg(target_os = "linux")]
            let available_filesystem_objects = capacity.f_ffree;
            #[cfg(not(target_os = "linux"))]
            let available_filesystem_objects = capacity.f_favail;
            capacity_from_values(
                capacity.f_blocks,
                capacity.f_bavail,
                capacity.f_frsize,
                capacity.f_files,
                available_filesystem_objects,
            )
        }
    })
    .await
    .map_err(|error| {
        FilesystemStorageError::io(
            "observe agent filesystem capacity",
            &root,
            std::io::Error::other(error),
        )
    })?
    .map_err(|error| FilesystemStorageError::io("observe agent filesystem capacity", &root, error))
}

#[cfg(not(target_os = "linux"))]
#[allow(
    dead_code,
    reason = "fresh physical capacity is part of the runtime filesystem interface"
)]
pub(super) async fn observe_path_capacity(
    root: &Path,
) -> Result<FilesystemCapacity, FilesystemStorageError> {
    Err(FilesystemStorageError::io(
        "observe agent filesystem capacity",
        root,
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "filesystem capacity observation requires statvfs",
        ),
    ))
}

pub(super) fn capacity_from_values(
    blocks: u64,
    available_blocks: u64,
    fragment_size: u64,
    filesystem_objects: u64,
    available_filesystem_objects: u64,
) -> std::io::Result<FilesystemCapacity> {
    let total_bytes = blocks.checked_mul(fragment_size).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "filesystem total capacity exceeds u64",
        )
    })?;
    let available_bytes = available_blocks.checked_mul(fragment_size).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "filesystem available capacity exceeds u64",
        )
    })?;
    Ok(FilesystemCapacity {
        total_bytes,
        available_bytes,
        total_filesystem_objects: filesystem_objects,
        available_filesystem_objects,
    })
}
