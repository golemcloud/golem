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
use crate::model::api_definition::UnboundCompiledRoute;
use anyhow::{Context, anyhow};
use golem_common::model::component::{AgentFilePermissions, InitialAgentFile};
use golem_service_base::custom_api::{RouteBehaviour, RouterFileIndexEntry};
use std::collections::HashSet;

/// Indexes the selected revision's read-only provisioning metadata without reading blobs.
/// Upload computes the content hash and size; the hash is also the immutable HTTP validator.
/// This index does not represent the agent's live filesystem.
pub(super) fn prepare_router_file_indexes(
    context: &DeploymentContext,
    routes: &mut [UnboundCompiledRoute],
) -> Result<(), DeploymentWriteError> {
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
        router.file_index = build(files)?;
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
}

fn build(files: &[InitialAgentFile]) -> Result<Vec<RouterFileIndexEntry>, DeploymentWriteError> {
    let mut paths = HashSet::new();
    for file in files {
        if !paths.insert(&file.path) {
            return Err(DeploymentWriteError::DuplicateRouterFileTarget);
        }
    }
    let mut index: Vec<_> = files
        .iter()
        .filter(|file| file.permissions == AgentFilePermissions::ReadOnly)
        .map(|file| RouterFileIndexEntry {
            path: file.path.to_string(),
            blob_key: file.content_hash,
            size: file.size,
        })
        .collect();
    index.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::AgentFileContentHash;
    use golem_common::model::path::AgentFilePath;
    use test_r::test;

    #[test]
    fn router_index_uses_upload_metadata_filters_read_write_and_rejects_duplicate_targets() {
        let ro = InitialAgentFile {
            path: AgentFilePath::from_abs_str("/public/a").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            content_hash: AgentFileContentHash(golem_common::model::diff::Hash::from(
                blake3::hash(b"blob"),
            )),
            size: 4_294_967_301,
        };
        let mut rw = ro.clone();
        rw.permissions = AgentFilePermissions::ReadWrite;
        assert!(matches!(
            build(&[ro.clone(), rw.clone()]),
            Err(DeploymentWriteError::DuplicateRouterFileTarget)
        ));
        rw.path = AgentFilePath::from_abs_str("/private/b").unwrap();
        let mut alias = ro.clone();
        alias.path = AgentFilePath::from_abs_str("/public/c").unwrap();
        let index = build(&[alias, rw, ro.clone()]).unwrap();
        assert_eq!(
            index
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/public/a", "/public/c"]
        );
        for entry in index {
            assert_eq!(entry.blob_key, ro.content_hash);
            assert_eq!(entry.size, 4_294_967_301);
        }
    }
}
