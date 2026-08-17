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
