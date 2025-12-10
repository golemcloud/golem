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
use crate::model::component::{show_exported_agents, ComponentDeployProperties};
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::text::component::is_sensitive_env_var_name;
use golem_client::model::{DeploymentPlan, DeploymentSummary};
use golem_common::model::agent::AgentType;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{
    CurrentDeploymentRevision, DeploymentPlanComponentEntry, DeploymentPlanHttpApiDefintionEntry,
    DeploymentPlanHttpApiDeploymentEntry,
};
use golem_common::model::diff;
use golem_common::model::diff::{Diffable, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap};

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
            .map(|d| &d.deployment_hash)
    }

    pub fn is_up_to_date(&self) -> bool {
        self.remote_deployment_hash() == Some(&self.local_deployment_hash)
    }
}

#[derive(Debug)]
pub struct DeployDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub deployable_components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub deployable_http_api_definitions: BTreeMap<HttpApiDefinitionName, app::HttpApiDefinition>,
    pub deployable_http_api_deployments: BTreeMap<Domain, Vec<HttpApiDefinitionName>>,
    pub diffable_local_deployment: diff::Deployment,
    pub local_deployment_hash: diff::Hash,
    #[allow(unused)] // NOTE: for debug logs
    pub server_deployment: Option<DeploymentSummary>,
    pub diffable_server_deployment: diff::Deployment,
    pub server_deployment_hash: diff::Hash,
    pub server_agent_types: HashMap<String, Vec<AgentType>>,
    pub staged_deployment: DeploymentPlan,
    pub staged_deployment_hash: diff::Hash,
    pub staged_agent_types: HashMap<String, Vec<AgentType>>,
    pub diffable_staged_deployment: diff::Deployment,
    pub diff: diff::DeploymentDiff,
    pub diff_stage: Option<diff::DeploymentDiff>,
}

impl DeployDiff {
    pub fn is_stage_same_as_server(&self) -> bool {
        self.staged_deployment_hash == self.server_deployment_hash
    }

    pub fn unified_diffs(&self, show_sensitive: bool) -> UnifiedDiffs {
        let local_for_stage = self.normalized_diff_deployment(
            show_sensitive,
            &self.diffable_local_deployment,
            self.diff_stage.as_ref(),
        );

        let staged_deployment = self.normalized_diff_deployment(
            show_sensitive,
            &self.diffable_staged_deployment,
            self.diff_stage.as_ref(),
        );

        let local_for_server = self.normalized_diff_deployment(
            show_sensitive,
            &self.diffable_local_deployment,
            Some(&self.diff),
        );

        let server_deployment = self.normalized_diff_deployment(
            show_sensitive,
            &self.diffable_server_deployment,
            Some(&self.diff),
        );

        let local_agents_for_stage = self
            .diff_stage
            .as_ref()
            .map(|diff_stage| {
                self.deployable_components
                    .iter()
                    .filter(|(component_name, _)| {
                        diff_stage.components.contains_key(&component_name.0)
                    })
                    .map(|(component_name, component)| {
                        self.render_component_agents(component_name, &component.agent_types)
                    })
                    .join("\n")
            })
            .unwrap_or_default();

        let staged_agents = self
            .diff_stage
            .as_ref()
            .map(|diff_stage| {
                diff_stage
                    .components
                    .iter()
                    .filter_map(|(component_name, _)| {
                        self.staged_agent_types
                            .get(component_name)
                            .map(|agent_types| {
                                self.render_component_agents(component_name, agent_types)
                            })
                    })
                    .join("\n")
            })
            .unwrap_or_default();

        let local_agents_for_server = self
            .deployable_components
            .iter()
            .filter(|(component_name, _)| self.diff.components.contains_key(&component_name.0))
            .map(|(component_name, component)| {
                self.render_component_agents(component_name, &component.agent_types)
            })
            .join("\n");

        let server_agents = self
            .diff
            .components
            .iter()
            .filter_map(|(component_name, _)| {
                self.server_agent_types
                    .get(component_name)
                    .map(|agent_types| self.render_component_agents(component_name, agent_types))
            })
            .join("\n");

        UnifiedDiffs {
            deployment_diff_stage: self.diff_stage.is_some().then(|| {
                staged_deployment.unified_yaml_diff_with_local(
                    &local_for_stage,
                    diff::SerializeMode::ValueIfAvailable,
                )
            }),
            agent_diff_stage: {
                let diff = diff::unified_diff(staged_agents, local_agents_for_stage);
                (!diff.is_empty()).then_some(diff)
            },
            deployment_diff: server_deployment.unified_yaml_diff_with_local(
                &local_for_server,
                diff::SerializeMode::ValueIfAvailable,
            ),
            agent_diff: {
                let diff = diff::unified_diff(server_agents, local_agents_for_server);
                (!diff.is_empty()).then_some(diff)
            },
        }
    }

    pub fn deployable_manifest_component(
        &self,
        component_name: &ComponentName,
    ) -> &ComponentDeployProperties {
        self.deployable_components
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
        self.staged_deployment
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
        self.staged_deployment
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
        self.staged_deployment
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

    pub fn current_deployment_revision(&self) -> Option<CurrentDeploymentRevision> {
        self.environment
            .remote_environment
            .current_deployment
            .as_ref()
            .map(|deployment| deployment.revision)
    }

    // Removes entries that are not involved in the diff and optionally masks sensitive values.
    fn normalized_diff_deployment(
        &self,
        show_sensitive: bool,
        deployment: &diff::Deployment,
        diff: Option<&diff::DeploymentDiff>,
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
                .filter(|(component_name, _)| {
                    diff.is_some_and(|diff| diff.components.contains_key(*component_name))
                })
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
            http_api_definitions: deployment
                .http_api_definitions
                .iter()
                .filter(|(http_api_definition_name, _)| {
                    diff.is_some_and(|diff| {
                        diff.http_api_definitions
                            .contains_key(*http_api_definition_name)
                    })
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            http_api_deployments: deployment
                .http_api_deployments
                .iter()
                .filter(|(domain, _)| {
                    diff.is_some_and(|diff| diff.http_api_deployments.contains_key(*domain))
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    fn render_component_agents(
        &self,
        component_name: impl AsRef<str>,
        agents: &[AgentType],
    ) -> String {
        format!(
            "{}:\n{}\n",
            component_name.as_ref(),
            show_exported_agents(agents, false, false)
                .into_iter()
                .map(|l| format!("  {l}"))
                .join("\n")
        )
    }
}

pub struct UnifiedDiffs {
    pub deployment_diff_stage: Option<String>,
    pub agent_diff_stage: Option<String>,
    pub deployment_diff: String,
    pub agent_diff: Option<String>,
}
