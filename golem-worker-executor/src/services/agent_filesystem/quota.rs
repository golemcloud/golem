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
    pub(crate) async fn capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError> {
        #[cfg(target_os = "linux")]
        if let Some(backend) = &self.managed_xfs {
            let backend = Arc::clone(backend);
            let root = backend.root().to_path_buf();
            return tokio::task::spawn_blocking(move || backend.capacity())
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed XFS capacity",
                        &root,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("observe managed XFS capacity", &root, error)
                });
        }

        Err(FilesystemStorageError::verification(
            "observe capacity for unmanaged filesystem storage",
            Path::new("<unmanaged>"),
        ))
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
        match &self.storage {
            AgentFilesystemStorage::Unmanaged => Ok(None),
            #[cfg(target_os = "linux")]
            AgentFilesystemStorage::Managed {
                backend,
                project_id,
                ..
            } => {
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                let path = self.path.clone();
                tokio::task::spawn_blocking(move || backend.usage(project_id))
                    .await
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS project usage",
                            &path,
                            std::io::Error::other(error),
                        )
                    })?
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS project usage",
                            &path,
                            error,
                        )
                    })
                    .map(Some)
            }
        }
    }

    pub(crate) async fn settle_reconstruction(&self) -> Result<(), FilesystemStorageError> {
        let _effect = self.runtime.begin_update_effect().await.map_err(|error| {
            FilesystemStorageError::io(
                "settle reconstructed agent filesystem",
                &self.path,
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
                self.inner.materializer.root(),
            ));
        }
        self.inner.materializer.usage().await
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
    pub(crate) async fn capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError> {
        self.inner.materializer.capacity().await
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
        let (observations, capacity) = tokio::join!(
            self.inner
                .materializer
                .failure_observations(installed_limits),
            self.capacity()
        );
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
                self.inner.materializer.root(),
                std::io::Error::other(error),
            )
        })?;
        let resolved = match self.inner.materializer {
            AgentFilesystemMaterializer::Unmanaged { .. } => return Ok(()),
            #[cfg(target_os = "linux")]
            AgentFilesystemMaterializer::Managed { .. } => {
                self.inner.filesystem_object_limit_policy.resolve(limit)?
            }
        };
        self.install_resolved_limits(resolved, effect.clone())
            .await?;
        drop(effect);
        Ok(())
    }

    async fn install_resolved_limits(
        &self,
        resolved: ResolvedAgentFilesystemLimits,
        effect: AgentFilesystemUpdateEffectLease,
    ) -> Result<(), FilesystemStorageError> {
        let usage = match self
            .inner
            .materializer
            .install_limits(resolved, effect)
            .await
        {
            Ok(usage) => usage,
            Err(error) => return Err(error),
        };
        *self
            .inner
            .applied_limits
            .write()
            .expect("agent filesystem applied-limit lock poisoned") = Some(resolved);
        let exceeded = usage.is_some_and(|usage| {
            usage.allocated_bytes > resolved.allocated_bytes
                || usage.filesystem_objects > resolved.filesystem_objects
        });
        self.notify_limit_state(exceeded).await;
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

impl AgentFilesystemMaterializer {
    async fn failure_observations(
        &self,
        installed_limits: Option<ResolvedAgentFilesystemLimits>,
    ) -> Result<
        (
            Option<AgentFilesystemUsage>,
            Option<ResolvedAgentFilesystemLimits>,
        ),
        FilesystemStorageError,
    > {
        match self {
            Self::Unmanaged { .. } => Ok((None, None)),
            #[cfg(target_os = "linux")]
            Self::Managed {
                root,
                backend,
                project_id,
            } => {
                let root = root.clone();
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                let policy_version = installed_limits
                    .map(|limits| limits.filesystem_object_limit_policy_version)
                    .unwrap_or(FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION);
                let observation_root = root.clone();
                let (usage, observed_limits) = tokio::task::spawn_blocking(move || {
                    backend.usage_and_limits(&observation_root, project_id, policy_version)
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "observe managed XFS project quota",
                        &root,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("observe managed XFS project quota", &root, error)
                })?;
                validate_observed_limits(&root, installed_limits, observed_limits)?;
                Ok((Some(usage), observed_limits))
            }
        }
    }

    async fn usage(&self) -> Result<Option<AgentFilesystemUsage>, FilesystemStorageError> {
        match self {
            Self::Unmanaged { .. } => Ok(None),
            #[cfg(target_os = "linux")]
            Self::Managed {
                root,
                backend,
                project_id,
            } => {
                let root = root.clone();
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                tokio::task::spawn_blocking(move || backend.usage(project_id))
                    .await
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS project usage",
                            &root,
                            std::io::Error::other(error),
                        )
                    })?
                    .map(Some)
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS project usage",
                            &root,
                            error,
                        )
                    })
            }
        }
    }

    #[allow(
        dead_code,
        reason = "fresh physical capacity is part of the runtime filesystem interface"
    )]
    async fn capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError> {
        match self {
            Self::Unmanaged { root } => observe_path_capacity(root).await,
            #[cfg(target_os = "linux")]
            Self::Managed { root, backend, .. } => {
                let root = root.clone();
                let backend = Arc::clone(backend);
                tokio::task::spawn_blocking(move || backend.capacity())
                    .await
                    .map_err(|error| {
                        FilesystemStorageError::io(
                            "observe managed XFS capacity",
                            &root,
                            std::io::Error::other(error),
                        )
                    })?
                    .map_err(|error| {
                        FilesystemStorageError::io("observe managed XFS capacity", &root, error)
                    })
            }
        }
    }

    pub(super) async fn install_limits(
        &self,
        limits: ResolvedAgentFilesystemLimits,
        effect: AgentFilesystemUpdateEffectLease,
    ) -> Result<Option<AgentFilesystemUsage>, FilesystemStorageError> {
        match self {
            Self::Unmanaged { .. } => Ok(None),
            #[cfg(target_os = "linux")]
            Self::Managed {
                root,
                backend,
                project_id,
                ..
            } => {
                let root = root.clone();
                let backend = Arc::clone(backend);
                let project_id = *project_id;
                let usage = tokio::task::spawn_blocking(move || {
                    let _effect = effect;
                    backend.install_project_limits(project_id, limits)?;
                    backend.usage(project_id)
                })
                .await
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "install managed XFS project limits",
                        &root,
                        std::io::Error::other(error),
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io("install managed XFS project limits", &root, error)
                })?;
                Ok(Some(usage))
            }
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
async fn observe_path_capacity(root: &Path) -> Result<FilesystemCapacity, FilesystemStorageError> {
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
async fn observe_path_capacity(root: &Path) -> Result<FilesystemCapacity, FilesystemStorageError> {
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
