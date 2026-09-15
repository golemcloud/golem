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

use super::deployment_context::DeploymentContext;
use super::{DeployValidationError, DeploymentWriteError};
use crate::config::RouterFileIndexConfig;
use crate::model::api_definition::UnboundCompiledRoute;
use anyhow::{Context, anyhow};
use futures::StreamExt;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::component::{AgentFilePermissions, InitialAgentFile};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::{RouteBehaviour, RouterFileIndexEntry};
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(super) struct RouterFileIndexBuilder {
    files: Arc<InitialAgentFilesService>,
    config: RouterFileIndexConfig,
    builds: Semaphore,
}

impl RouterFileIndexBuilder {
    pub fn new(files: Arc<InitialAgentFilesService>, config: RouterFileIndexConfig) -> Self {
        assert!(
            config.max_concurrent_builds > 0,
            "Router file index concurrency must be positive"
        );
        assert!(
            !config.timeout.is_zero(),
            "Router file index timeout must be positive"
        );
        Self {
            files,
            builds: Semaphore::new(config.max_concurrent_builds),
            config,
        }
    }

    pub async fn prepare(
        &self,
        context: &DeploymentContext,
        routes: &mut [UnboundCompiledRoute],
    ) -> Result<(), DeploymentWriteError> {
        if !routes
            .iter()
            .any(|route| matches!(route.behaviour, RouteBehaviour::HttpRouter(_)))
        {
            return Ok(());
        }
        let _permit = self
            .builds
            .try_acquire()
            .map_err(|_| DeploymentWriteError::RouterFileIndexBusy)?;
        tokio::time::timeout(self.config.timeout, async {
            let mut blobs = HashMap::new();
            for route in routes {
                let RouteBehaviour::HttpRouter(router) = &mut route.behaviour else {
                    continue;
                };
                let component = context
                    .components
                    .values()
                    .find(|component| {
                        component.id == router.component_id
                            && component.revision == router.component_revision
                    })
                    .ok_or_else(|| anyhow!("Router component missing from selected deployment"))?;
                let files = component
                    .metadata
                    .agent_type_provision_configs()
                    .get(&router.agent_type)
                    .map(|config| config.files.as_slice())
                    .context("Router provisioning missing from selected component")?;
                router.file_index = self
                    .build(context.environment.id, files, &mut blobs)
                    .await?;
                route
                    .route_match
                    .validate(&route.path, &route.behaviour)
                    .map_err(|error| {
                        DeploymentWriteError::DeploymentValidationFailed(vec![
                            DeployValidationError::HttpApiDeploymentInvalidRoute {
                                domain: route.domain.clone(),
                                path: route.path.clone(),
                                error,
                            },
                        ])
                    })?;
            }
            Ok(())
        })
        .await
        .map_err(|_| DeploymentWriteError::RouterFileIndexTimeout)?
    }

    async fn build(
        &self,
        environment: EnvironmentId,
        files: &[InitialAgentFile],
        blobs: &mut HashMap<AgentFileContentHash, (u64, [u8; 32])>,
    ) -> Result<Vec<RouterFileIndexEntry>, DeploymentWriteError> {
        let mut paths = HashSet::new();
        for file in files {
            if !paths.insert(&file.path) {
                return Err(DeploymentWriteError::DuplicateRouterFileTarget);
            }
            tokio::task::yield_now().await;
        }
        let mut index = Vec::new();
        for file in files
            .iter()
            .filter(|file| file.permissions == AgentFilePermissions::ReadOnly)
        {
            let (size, sha256) = match blobs.get(&file.content_hash) {
                Some(value) => *value,
                None => {
                    let value = self.hash_blob(environment, file.content_hash).await?;
                    blobs.insert(file.content_hash, value);
                    value
                }
            };
            if size != file.size {
                return Err(anyhow!("Router initial-file size does not match stored blob").into());
            }
            index.push(RouterFileIndexEntry {
                path: file.path.to_string(),
                blob_key: file.content_hash,
                size,
                sha256,
            });
            tokio::task::yield_now().await;
        }
        index.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(index)
    }

    async fn hash_blob(
        &self,
        environment: EnvironmentId,
        key: AgentFileContentHash,
    ) -> anyhow::Result<(u64, [u8; 32])> {
        let mut stream = self
            .files
            .get(environment, key)
            .await?
            .context("Router initial-file blob missing")?;
        let mut size = 0u64;
        let mut sha256 = Sha256::new();
        let mut blake3 = blake3::Hasher::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            tokio::task::yield_now().await;
            for part in chunk.chunks(64 * 1024) {
                size = size
                    .checked_add(part.len() as u64)
                    .context("Router file length overflow")?;
                sha256.update(part);
                blake3.update(part);
                tokio::task::yield_now().await;
            }
        }
        if blake3.finalize() != *key.0.as_blake3_hash() {
            return Err(anyhow!(
                "Router initial-file content identity does not match stored blob"
            ));
        }
        Ok((size, sha256.finalize().into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::path::AgentFilePath;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
    use test_r::test;

    #[test]
    async fn router_index_hashes_bounded_chunks_and_rejects_inconsistent_blobs() {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let files = Arc::new(InitialAgentFilesService::new(storage.clone()));
        let builder = RouterFileIndexBuilder::new(files.clone(), RouterFileIndexConfig::default());
        let environment = EnvironmentId::new();
        for (content, expected) in [
            (
                b"abc".to_vec(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                vec![b'a'; 1_000_000],
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ] {
            let size = content.len() as u64;
            let key = files
                .put_if_not_exists(
                    environment,
                    content
                        .map_item(|item| item.map_err(anyhow::Error::from))
                        .map_error(anyhow::Error::from),
                )
                .await
                .unwrap();
            let file = InitialAgentFile {
                path: AgentFilePath::from_abs_str("/public/a").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
                content_hash: key,
                size,
            };
            let mut cache = HashMap::new();
            let index = builder
                .build(environment, &[file.clone()], &mut cache)
                .await
                .unwrap();
            assert_eq!(index[0].size, size);
            assert_eq!(
                index[0]
                    .sha256
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                expected
            );
            assert_eq!(index[0].blob_key, key);
            let mut wrong_size = file.clone();
            wrong_size.size += 1;
            assert!(
                builder
                    .build(environment, &[wrong_size], &mut cache)
                    .await
                    .is_err()
            );
            assert!(
                builder
                    .build(EnvironmentId::new(), &[file.clone()], &mut HashMap::new())
                    .await
                    .is_err()
            );
            storage
                .put_raw(
                    "test",
                    "corrupt",
                    BlobStorageNamespace::InitialAgentFiles {
                        environment_id: environment,
                    },
                    &std::path::PathBuf::from(key.0.as_blake3_hash().to_hex().to_string()),
                    b"corrupt",
                )
                .await
                .unwrap();
            // A new deployment must read again, not reuse another build's hash cache.
            assert!(
                builder
                    .build(environment, &[file], &mut HashMap::new())
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    async fn router_index_filters_read_write_and_rejects_duplicate_targets() {
        let files = Arc::new(InitialAgentFilesService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        let builder = RouterFileIndexBuilder::new(files.clone(), RouterFileIndexConfig::default());
        let environment = EnvironmentId::new();
        let key = files
            .put_if_not_exists(
                environment,
                b"abc"
                    .to_vec()
                    .map_item(|item| item.map_err(anyhow::Error::from))
                    .map_error(anyhow::Error::from),
            )
            .await
            .unwrap();
        let ro = InitialAgentFile {
            path: AgentFilePath::from_abs_str("/public/a").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            content_hash: key,
            size: 3,
        };
        let mut rw = ro.clone();
        rw.permissions = AgentFilePermissions::ReadWrite;
        assert!(matches!(
            builder
                .build(environment, &[ro.clone(), rw.clone()], &mut HashMap::new())
                .await,
            Err(DeploymentWriteError::DuplicateRouterFileTarget)
        ));
        rw.path = AgentFilePath::from_abs_str("/private/b").unwrap();
        rw.content_hash = AgentFileContentHash(golem_common::model::diff::Hash::empty());
        let mut alias = ro.clone();
        alias.path = AgentFilePath::from_abs_str("/public/c").unwrap();
        let mut cache = HashMap::new();
        let index = builder
            .build(environment, &[alias, rw, ro], &mut cache)
            .await
            .unwrap();
        assert_eq!(
            index
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/public/a", "/public/c"]
        );
        assert_eq!(cache.len(), 1);
    }
}
