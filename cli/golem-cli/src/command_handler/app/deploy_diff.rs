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

use crate::model::component::{show_exported_agents, ComponentDeployProperties};
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::http_api::HttpApiDeploymentDeployProperties;
use crate::model::text::component::is_sensitive_env_var_name;
use anyhow::bail;
use golem_client::model::{DeploymentPlan, DeploymentSummary};
use golem_common::model::agent::AgentType;
use golem_common::model::component::{ComponentDto, ComponentName};
use golem_common::model::deployment::{
    CurrentDeploymentRevision, DeploymentPlanComponentEntry, DeploymentPlanHttpApiDeploymentEntry,
};
use golem_common::model::diff;
use golem_common::model::diff::{Diffable, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentCurrentDeploymentView;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap};
use tracing::debug;

#[derive(Debug)]
pub struct DeployQuickDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub deployable_manifest_components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub deployable_manifest_http_api_deployments:
        BTreeMap<Domain, HttpApiDeploymentDeployProperties>,
    pub diffable_local_deployment: diff::Deployment,
    pub local_deployment_hash: diff::Hash,
}

impl DeployQuickDiff {
    pub fn current_deployment_hash(&self) -> Option<&diff::Hash> {
        self.environment
            .server_environment
            .current_deployment
            .as_ref()
            .map(|d| &d.deployment_hash)
    }

    pub fn is_up_to_date(&self) -> bool {
        let current_deployment_hash = self.current_deployment_hash();
        debug!(
            current_deployment_hash = current_deployment_hash
                .map(|s| s.to_string())
                .unwrap_or("-".to_string()),
            local_deployment_hash = self.local_deployment_hash.to_string(),
            "is_up_to_date"
        );
        current_deployment_hash == Some(&self.local_deployment_hash)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DeployDiffKind {
    Stage,
    Current,
}

#[derive(Debug)]
pub struct DeployDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub deployable_components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub deployable_http_api_deployments: BTreeMap<Domain, HttpApiDeploymentDeployProperties>,
    pub diffable_local_deployment: diff::Deployment,
    pub local_deployment_hash: diff::Hash,
    #[allow(unused)] // NOTE: for debug logs
    pub current_deployment: Option<DeploymentSummary>,
    pub diffable_current_deployment: diff::Deployment,
    pub current_deployment_hash: diff::Hash,
    pub current_agent_types: HashMap<String, Vec<AgentType>>,
    pub staged_deployment: DeploymentPlan,
    pub staged_deployment_hash: diff::Hash,
    pub staged_agent_types: HashMap<String, Vec<AgentType>>,
    pub diffable_staged_deployment: diff::Deployment,
    pub diff: diff::DeploymentDiff,
    pub diff_stage: Option<diff::DeploymentDiff>,
}

impl DeployDiff {
    pub fn is_stage_same_as_current(&self) -> bool {
        self.staged_deployment_hash == self.current_deployment_hash
    }

    pub fn unified_diffs(&self, show_sensitive: bool) -> DeployUnifiedDiffs {
        let local_for_stage = normalized_diff_deployment(
            show_sensitive,
            &self.diffable_local_deployment,
            self.diff_stage.as_ref(),
        );

        let staged_deployment = normalized_diff_deployment(
            show_sensitive,
            &self.diffable_staged_deployment,
            self.diff_stage.as_ref(),
        );

        let local_for_current = normalized_diff_deployment(
            show_sensitive,
            &self.diffable_local_deployment,
            Some(&self.diff),
        );

        let current_deployment = normalized_diff_deployment(
            show_sensitive,
            &self.diffable_current_deployment,
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
                        format_component_agents_for_diff(component_name, &component.agent_types)
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
                                format_component_agents_for_diff(component_name, agent_types)
                            })
                    })
                    .join("\n")
            })
            .unwrap_or_default();

        let local_agents_for_current = self
            .deployable_components
            .iter()
            .filter(|(component_name, _)| self.diff.components.contains_key(&component_name.0))
            .map(|(component_name, component)| {
                format_component_agents_for_diff(component_name, &component.agent_types)
            })
            .join("\n");

        let current_agents = self
            .diff
            .components
            .iter()
            .filter_map(|(component_name, _)| {
                self.current_agent_types
                    .get(component_name)
                    .map(|agent_types| {
                        format_component_agents_for_diff(component_name, agent_types)
                    })
            })
            .join("\n");

        DeployUnifiedDiffs {
            deployment_diff_stage: self.diff_stage.is_some().then(|| {
                staged_deployment.unified_yaml_diff_with_new(
                    &local_for_stage,
                    diff::SerializeMode::ValueIfAvailable,
                )
            }),
            agent_diff_stage: {
                let diff = diff::unified_diff(staged_agents, local_agents_for_stage);
                (!diff.is_empty()).then_some(diff)
            },
            deployment_diff: current_deployment.unified_yaml_diff_with_new(
                &local_for_current,
                diff::SerializeMode::ValueIfAvailable,
            ),
            agent_diff: {
                let diff = diff::unified_diff(current_agents, local_agents_for_current);
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
                    "Expected component {} not found in deployable manifest",
                    component_name
                )
            })
    }

    pub fn deployable_manifest_http_api_deployment(
        &self,
        domain: &Domain,
    ) -> &HttpApiDeploymentDeployProperties {
        self.deployable_http_api_deployments
            .get(domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API deployment {} not found in deployable manifest",
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
                    "Expected component {} not found in staged deployment",
                    component_name
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
                    "Expected HTTP API deployment {} not found in staged deployment",
                    domain
                )
            })
    }

    pub fn current_component_identity(
        &self,
        component_name: &ComponentName,
    ) -> &DeploymentPlanComponentEntry {
        self.current_deployment
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "Expected component {}  not found in current deployment, no deployment",
                    component_name
                )
            })
            .components
            .iter()
            .find(|component| &component.name == component_name)
            .unwrap_or_else(|| {
                panic!(
                    "Expected component {} not found in current deployment",
                    component_name
                )
            })
    }

    pub fn current_http_api_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &DeploymentPlanHttpApiDeploymentEntry {
        self.current_deployment
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API deployment {} not found in deployment plan, no deployment",
                    domain
                )
            })
            .http_api_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API deployment {} not found in current deployment",
                    domain
                )
            })
    }

    pub fn component_identity(
        &self,
        kind: DeployDiffKind,
        component_name: &ComponentName,
    ) -> &DeploymentPlanComponentEntry {
        match kind {
            DeployDiffKind::Stage => self.staged_component_identity(component_name),
            DeployDiffKind::Current => self.current_component_identity(component_name),
        }
    }

    pub fn http_api_deployment_identity(
        &self,
        kind: DeployDiffKind,
        domain: &Domain,
    ) -> &DeploymentPlanHttpApiDeploymentEntry {
        match kind {
            DeployDiffKind::Stage => self.staged_http_api_deployment_identity(domain),
            DeployDiffKind::Current => self.current_http_api_deployment_identity(domain),
        }
    }

    pub fn add_details(
        &mut self,
        kind: DeployDiffKind,
        details: DeployDetails,
    ) -> anyhow::Result<()> {
        for (component_name, component) in details.component {
            self.add_component_details(kind, component_name, component);
        }

        for (domain, http_api_deployment) in details.http_api_deployment {
            self.add_http_api_deployment_details(kind, domain, http_api_deployment)
        }

        match kind {
            DeployDiffKind::Stage => {
                self.diff_stage = self
                    .diffable_staged_deployment
                    .diff_with_new(&self.diffable_local_deployment);
            }
            DeployDiffKind::Current => {
                match self
                    .diffable_current_deployment
                    .diff_with_new(&self.diffable_local_deployment)
                {
                    Some(diff) => self.diff = diff,
                    None => {
                        bail!("Illegal state: empty diff between current and local deployment after adding details")
                    }
                }
            }
        }

        Ok(())
    }

    fn add_component_details(
        &mut self,
        kind: DeployDiffKind,
        component_name: ComponentName,
        component: ComponentDto,
    ) {
        match kind {
            DeployDiffKind::Stage => {
                self.diffable_staged_deployment
                    .components
                    .insert(component_name.0.clone(), component.to_diffable().into());
                self.staged_agent_types
                    .insert(component_name.0, component.metadata.agent_types().to_vec());
            }
            DeployDiffKind::Current => {
                self.diffable_current_deployment
                    .components
                    .insert(component_name.0.clone(), component.to_diffable().into());
                self.current_agent_types
                    .insert(component_name.0, component.metadata.agent_types().to_vec());
            }
        }
    }

    fn add_http_api_deployment_details(
        &mut self,
        kind: DeployDiffKind,
        domain: Domain,
        http_api_deployment: HttpApiDeployment,
    ) {
        match &kind {
            DeployDiffKind::Stage => {
                self.diffable_staged_deployment
                    .http_api_deployments
                    .insert(domain.0, http_api_deployment.to_diffable().into());
            }
            DeployDiffKind::Current => {
                self.diffable_current_deployment
                    .http_api_deployments
                    .insert(domain.0, http_api_deployment.to_diffable().into());
            }
        }
    }

    pub fn current_deployment_revision(&self) -> Option<CurrentDeploymentRevision> {
        self.environment
            .server_environment
            .current_deployment
            .as_ref()
            .map(|deployment| deployment.revision)
    }
}

#[derive(Debug)]
pub struct DeployDetails {
    pub component: Vec<(ComponentName, ComponentDto)>,
    pub http_api_deployment: Vec<(Domain, HttpApiDeployment)>,
}

#[derive(Debug)]
pub struct DeployUnifiedDiffs {
    pub deployment_diff_stage: Option<String>,
    pub agent_diff_stage: Option<String>,
    pub deployment_diff: String,
    pub agent_diff: Option<String>,
}

#[derive(Debug)]
pub struct RollbackQuickDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub current_deployment_meta: EnvironmentCurrentDeploymentView,
    pub target_deployment: DeploymentSummary,
}

impl RollbackQuickDiff {
    pub fn is_target_same_as_current(&self) -> bool {
        debug!(
            current_deployment_hash = self.current_deployment_meta.deployment_hash.to_string(),
            target_deployment_hash = self.target_deployment.deployment_hash.to_string(),
            "is_up_to_date"
        );
        self.current_deployment_meta.deployment_hash == self.target_deployment.deployment_hash
    }
}

#[derive(Debug)]
pub struct RollbackDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub current_deployment_meta: EnvironmentCurrentDeploymentView,
    pub target_deployment: DeploymentSummary,
    pub current_deployment: DeploymentSummary,
    pub diffable_target_deployment: diff::Deployment,
    pub diffable_current_deployment: diff::Deployment,
    pub current_agent_types: HashMap<String, Vec<AgentType>>,
    pub target_agent_types: HashMap<String, Vec<AgentType>>,
    pub diff: diff::DeploymentDiff,
}

impl RollbackDiff {
    pub fn current_component_identity(
        &self,
        component_name: &ComponentName,
    ) -> &DeploymentPlanComponentEntry {
        self.current_deployment
            .components
            .iter()
            .find(|component| &component.name == component_name)
            .unwrap_or_else(|| {
                panic!(
                    "Expected component {} not found in current deployment",
                    component_name
                )
            })
    }

    pub fn current_http_api_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &DeploymentPlanHttpApiDeploymentEntry {
        self.current_deployment
            .http_api_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API deployment {} not found in current deployment",
                    domain
                )
            })
    }

    pub fn target_component_identity(
        &self,
        component_name: &ComponentName,
    ) -> &DeploymentPlanComponentEntry {
        self.target_deployment
            .components
            .iter()
            .find(|component| &component.name == component_name)
            .unwrap_or_else(|| {
                panic!(
                    "Expected component {} not found in target deployment",
                    component_name
                )
            })
    }

    pub fn target_http_api_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &DeploymentPlanHttpApiDeploymentEntry {
        self.target_deployment
            .http_api_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected HTTP API deployment {} not found in target deployment",
                    domain
                )
            })
    }

    fn add_component_details(
        &mut self,
        component_details: RollbackEntityDetails<ComponentName, ComponentDto>,
    ) {
        if let Some(component) = component_details.new {
            self.diffable_target_deployment.components.insert(
                component_details.name.0.clone(),
                component.to_diffable().into(),
            );
            self.target_agent_types.insert(
                component_details.name.0.clone(),
                component.metadata.agent_types().to_vec(),
            );
        }
        if let Some(component) = component_details.current {
            self.diffable_current_deployment.components.insert(
                component_details.name.0.clone(),
                component.to_diffable().into(),
            );
            self.current_agent_types.insert(
                component_details.name.0.clone(),
                component.metadata.agent_types().to_vec(),
            );
        }
    }

    fn add_http_api_deployment_details(
        &mut self,
        http_api_deployment_details: RollbackEntityDetails<Domain, HttpApiDeployment>,
    ) {
        if let Some(http_api_deployment) = http_api_deployment_details.new {
            self.diffable_target_deployment.http_api_deployments.insert(
                http_api_deployment_details.name.0.clone(),
                http_api_deployment.to_diffable().into(),
            );
        }
        if let Some(http_api_deployment) = http_api_deployment_details.current {
            self.diffable_current_deployment
                .http_api_deployments
                .insert(
                    http_api_deployment_details.name.0.clone(),
                    http_api_deployment.to_diffable().into(),
                );
        }
    }

    pub fn add_details(&mut self, details: RollbackDetails) -> anyhow::Result<()> {
        for component_details in details.component {
            self.add_component_details(component_details);
        }

        for http_api_deployment_details in details.http_api_deployment {
            self.add_http_api_deployment_details(http_api_deployment_details);
        }

        match self
            .diffable_target_deployment
            .diff_with_current(&self.diffable_current_deployment)
        {
            Some(diff) => {
                self.diff = diff;
            }
            None => {
                bail!("Illegal state: empty diff between taget and current deployment after adding details");
            }
        }

        Ok(())
    }

    pub fn unified_diffs(&self, show_sensitive: bool) -> RollbackUnifiedDiffs {
        let target_deployment = normalized_diff_deployment(
            show_sensitive,
            &self.diffable_target_deployment,
            Some(&self.diff),
        );

        let current_deployment = normalized_diff_deployment(
            show_sensitive,
            &self.diffable_current_deployment,
            Some(&self.diff),
        );

        let target_agents = self
            .diff
            .components
            .iter()
            .filter_map(|(component_name, _)| {
                self.target_agent_types
                    .get(component_name)
                    .map(|agent_types| {
                        format_component_agents_for_diff(component_name, agent_types)
                    })
            })
            .join("\n");

        let current_agents = self
            .diff
            .components
            .iter()
            .filter_map(|(component_name, _)| {
                self.current_agent_types
                    .get(component_name)
                    .map(|agent_types| {
                        format_component_agents_for_diff(component_name, agent_types)
                    })
            })
            .join("\n");

        RollbackUnifiedDiffs {
            deployment_diff: target_deployment.unified_yaml_diff_with_current(
                &current_deployment,
                diff::SerializeMode::ValueIfAvailable,
            ),
            agent_diff: {
                let diff = diff::unified_diff(current_agents, target_agents);
                (!diff.is_empty()).then_some(diff)
            },
        }
    }
}

#[derive(Debug)]
pub struct RollbackEntityDetails<Name, Entity> {
    pub name: Name,
    pub new: Option<Entity>,
    pub current: Option<Entity>,
}

impl<'a, Name, Entity> RollbackEntityDetails<Name, &'a Entity> {
    pub fn new_identity<DiffValue>(
        name: Name,
        get_new: fn(&'a RollbackDiff, &Name) -> &'a Entity,
        get_current: fn(&'a RollbackDiff, &Name) -> &'a Entity,
        rollback_diff: &'a RollbackDiff,
        entity_diff: &diff::BTreeMapDiffValue<DiffValue>,
    ) -> Self {
        match entity_diff {
            diff::BTreeMapDiffValue::Create => Self {
                new: Some(get_new(rollback_diff, &name)),
                current: None,
                name,
            },
            diff::BTreeMapDiffValue::Delete => Self {
                new: None,
                current: Some(get_current(rollback_diff, &name)),
                name,
            },
            diff::BTreeMapDiffValue::Update(_) => Self {
                new: Some(get_new(rollback_diff, &name)),
                current: Some(get_current(rollback_diff, &name)),
                name,
            },
        }
    }
}

#[derive(Debug)]
pub struct RollbackDetails {
    pub component: Vec<RollbackEntityDetails<ComponentName, ComponentDto>>,
    pub http_api_deployment: Vec<RollbackEntityDetails<Domain, HttpApiDeployment>>,
}

#[derive(Debug)]
pub struct RollbackUnifiedDiffs {
    pub deployment_diff: String,
    pub agent_diff: Option<String>,
}

// Removes entries that are not involved in the diff and optionally masks sensitive values.
fn normalized_diff_deployment(
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
                                    config_vars: metadata.config_vars.clone(),
                                }
                                .into(),
                                None => component.metadata.hash().into(),
                            },
                            wasm_hash: component.wasm_hash,
                            files_by_path: component.files_by_path.clone(),
                            plugins_by_grant_id: component.plugins_by_grant_id.clone(),
                            local_agent_config_ordered_by_agent_and_key: component
                                .local_agent_config_ordered_by_agent_and_key
                                .clone(),
                        }
                        .into(),
                        None => component.hash().into(),
                    },
                )
            })
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

fn format_component_agents_for_diff(
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
