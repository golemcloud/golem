// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::model::app;
use crate::model::component::ComponentDeployProperties;
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::text::component::is_sensitive_env_var_name;
use golem_client::model::{DeploymentPlan, DeploymentSummary};
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{
    DeploymentPlanComponentEntry, DeploymentPlanHttpApiDefintionEntry,
    DeploymentPlanHttpApiDeploymentEntry, DeploymentRevision,
};
use golem_common::model::diff;
use golem_common::model::diff::{Diffable, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct DeployQuickDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub deployable_manifest_components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub deployable_manifest_http_api_definitions:
        BTreeMap<HttpApiDefinitionName, app::HttpApiDefinition>,
    pub deployable_manifest_http_api_deployments: BTreeMap<Domain, Vec<HttpApiDefinitionName>>,
    pub diffable_local_deployment: diff::Deployment,
    pub local_deployment_hash: diff::Hash,
}

impl DeployQuickDiff {
    pub fn remote_deployment_hash(&self) -> Option<&diff::Hash> {
        self.environment
            .remote_environment
            .current_deployment
            .as_ref()
            .map(|d| &d.hash)
    }

    pub fn is_up_to_date(&self) -> bool {
        self.remote_deployment_hash() == Some(&self.local_deployment_hash)
    }
}

#[derive(Debug)]
pub struct DeployDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub deployable_manifest_components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub deployable_http_api_definitions: BTreeMap<HttpApiDefinitionName, app::HttpApiDefinition>,
    pub deployable_http_api_deployments: BTreeMap<Domain, Vec<HttpApiDefinitionName>>,
    pub diffable_local_deployment: diff::Deployment,
    pub local_deployment_hash: diff::Hash,
    #[allow(unused)] // NOTE: for debug logs
    pub server_deployment: Option<DeploymentSummary>,
    pub diffable_server_deployment: diff::Deployment,
    pub server_deployment_hash: diff::Hash,
    pub server_staged_deployment: DeploymentPlan,
    pub server_staged_deployment_hash: diff::Hash,
    pub diffable_staged_deployment: diff::Deployment,
    pub diff: diff::DeploymentDiff,
    pub diff_stage: Option<diff::DeploymentDiff>,
}

impl DeployDiff {
    pub fn is_stage_same_as_server(&self) -> bool {
        self.server_staged_deployment_hash == self.server_deployment_hash
    }

    pub fn unified_yaml_diffs(&self, show_sensitive: bool) -> UnifiedYamlDeployDiff {
        let safe_diffable_local_deployment =
            Self::safe_diff_deployment(show_sensitive, &self.diffable_local_deployment);
        let safe_diffable_server_deployment =
            Self::safe_diff_deployment(show_sensitive, &self.diffable_server_deployment);
        let safe_diffable_server_staged_deployment =
            Self::safe_diff_deployment(show_sensitive, &self.diffable_staged_deployment);

        UnifiedYamlDeployDiff {
            diff_stage: self.diff_stage.is_some().then(|| {
                safe_diffable_server_staged_deployment.unified_yaml_diff_with_local(
                    &safe_diffable_local_deployment,
                    diff::SerializeMode::ValueIfAvailable,
                )
            }),
            diff: safe_diffable_server_deployment.unified_yaml_diff_with_local(
                &safe_diffable_local_deployment,
                diff::SerializeMode::ValueIfAvailable,
            ),
        }
    }

    pub fn deployable_manifest_component(
        &self,
        component_name: &ComponentName,
    ) -> &ComponentDeployProperties {
        self.deployable_manifest_components
            .get(component_name)
            .unwrap_or_else(|| {
                panic!(
                    "Illegal state, missing component {} from component deploy properties",
                    component_name
                )
            })
    }

    pub fn deployable_manifest_http_api_definition(
        &self,
        http_api_definition_name: &HttpApiDefinitionName,
    ) -> &app::HttpApiDefinition {
        self.deployable_http_api_definitions
            .get(http_api_definition_name)
            .unwrap_or_else(|| {
                panic!(
                    "Illegal state, missing HTTP API definition {} from deployable manifest",
                    http_api_definition_name
                )
            })
    }

    pub fn deployable_manifest_http_api_deployment(
        &self,
        domain: &Domain,
    ) -> &Vec<HttpApiDefinitionName> {
        self.deployable_http_api_deployments
            .get(domain)
            .unwrap_or_else(|| {
                panic!(
                    "Illegal state, missing HTTP API deployment {} from deployable manifest",
                    domain
                )
            })
    }

    pub fn staged_component_identity(
        &self,
        component_name: &ComponentName,
    ) -> &DeploymentPlanComponentEntry {
        self.server_staged_deployment
            .components
            .iter()
            .find(|component| &component.name == component_name)
            .unwrap_or_else(|| {
                panic!(
                    "Expected component {} not found in deployment plan",
                    component_name
                )
            })
    }

    pub fn staged_http_api_definition_identity(
        &self,
        http_api_definition_name: &HttpApiDefinitionName,
    ) -> &DeploymentPlanHttpApiDefintionEntry {
        self.server_staged_deployment
            .http_api_definitions
            .iter()
            .find(|def| &def.name == http_api_definition_name)
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API definition {} not found in deployment plan",
                    http_api_definition_name
                )
            })
    }

    pub fn staged_http_api_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &DeploymentPlanHttpApiDeploymentEntry {
        self.server_staged_deployment
            .http_api_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API deployment {} not found in deployment plan",
                    domain
                )
            })
    }

    pub fn current_deployment_revision(&self) -> Option<DeploymentRevision> {
        self.environment
            .remote_environment
            .current_deployment
            .as_ref()
            .map(|deployment| deployment.revision)
    }

    fn safe_diff_deployment(
        show_sensitive: bool,
        deployment: &diff::Deployment,
    ) -> diff::Deployment {
        if show_sensitive {
            return deployment.clone();
        }

        let safe_env = |env: &BTreeMap<String, String>| -> BTreeMap<String, String> {
            env.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if is_sensitive_env_var_name(show_sensitive, k) {
                            format!("<hashed-secret:{}>", blake3::hash(v.as_bytes()).to_hex())
                        } else {
                            v.clone()
                        },
                    )
                })
                .collect()
        };

        diff::Deployment {
            components: deployment
                .components
                .iter()
                .map(|(component_name, component)| {
                    (
                        component_name.clone(),
                        match component.as_value() {
                            Some(component) => diff::Component {
                                metadata: match component.metadata.as_value() {
                                    Some(metadata) => diff::ComponentMetadata {
                                        version: metadata.version.clone(),
                                        env: safe_env(&metadata.env),
                                        dynamic_linking_wasm_rpc: Default::default(),
                                    }
                                    .into(),
                                    None => component.metadata.hash().into(),
                                },
                                wasm_hash: component.wasm_hash,
                                files_by_path: component.files_by_path.clone(),
                                plugins_by_priority: component.plugins_by_priority.clone(),
                            }
                            .into(),
                            None => component.hash().into(),
                        },
                    )
                })
                .collect(),
            http_api_definitions: deployment.http_api_definitions.clone(),
            http_api_deployments: deployment.http_api_deployments.clone(),
        }
    }
}

pub struct UnifiedYamlDeployDiff {
    pub diff_stage: Option<String>,
    pub diff: String,
}
