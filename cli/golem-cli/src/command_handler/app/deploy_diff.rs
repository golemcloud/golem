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

use crate::model::component::ComponentDeployProperties;
use crate::model::deploy::{
    DeploymentDisplay, DeploymentDisplayContext, DeploymentDisplayMode, EnvironmentSetupPlan,
};
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::http_api::{HttpApiDeploymentDeployProperties, McpDeploymentDeployProperties};
use crate::model::masking::MaskingConfig;
use anyhow::bail;
use golem_client::model::{DeploymentPlan, DeploymentSummary};
use golem_common::model::component::{ComponentDto, ComponentName};
use golem_common::model::deployment::{
    CurrentDeploymentRevision, DeploymentPlanComponentEntry, DeploymentPlanHttpApiDeploymentEntry,
};
use golem_common::model::diff;
use golem_common::model::diff::Diffable;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentCurrentDeploymentView;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::mcp_deployment::McpDeployment;
use golem_common::schema::agent::AgentTypeSchema;
use std::collections::{BTreeMap, HashMap};
use tracing::debug;

#[derive(Debug)]
pub struct DeployQuickDiff {
    pub environment: ResolvedEnvironmentIdentity,
    pub deployable_manifest_components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub deployable_manifest_http_api_deployments:
        BTreeMap<Domain, HttpApiDeploymentDeployProperties>,
    #[allow(dead_code)]
    pub deployable_manifest_mcp_deployments: BTreeMap<Domain, McpDeploymentDeployProperties>,
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
    pub deployable_mcp_deployments: BTreeMap<Domain, McpDeploymentDeployProperties>,
    pub diffable_local_deployment: diff::Deployment,
    pub local_deployment_hash: diff::Hash,
    #[allow(unused)] // NOTE: for debug logs
    pub current_deployment: Option<DeploymentSummary>,
    pub diffable_current_deployment: diff::Deployment,
    pub current_deployment_hash: diff::Hash,
    pub current_agent_types: HashMap<String, Vec<AgentTypeSchema>>,
    pub staged_deployment: DeploymentPlan,
    pub staged_deployment_hash: diff::Hash,
    pub staged_agent_types: HashMap<String, Vec<AgentTypeSchema>>,
    pub diffable_staged_deployment: diff::Deployment,
    pub diff: diff::DeploymentDiff,
    pub diff_stage: Option<diff::DeploymentDiff>,
    pub environment_setup: Option<EnvironmentSetupPlan>,
}

impl DeployDiff {
    pub fn is_stage_same_as_current(&self) -> bool {
        self.staged_deployment_hash == self.current_deployment_hash
    }

    pub fn has_deployment_changes(&self) -> bool {
        !self.diff.components.is_empty()
            || !self.diff.http_api_deployments.is_empty()
            || !self.diff.mcp_deployments.is_empty()
    }

    pub fn has_environment_setup_entries_to_apply(&self) -> bool {
        self.environment_setup
            .as_ref()
            .is_some_and(|setup| setup.display.has_entries_to_apply())
    }

    pub fn has_environment_setup_entries_skipped_already_exists(&self) -> bool {
        self.environment_setup
            .as_ref()
            .is_some_and(|setup| setup.display.has_entries_skipped_already_exists())
    }

    pub fn has_environment_setup_work(&self) -> bool {
        self.has_environment_setup_entries_to_apply()
            || self.has_environment_setup_entries_skipped_already_exists()
    }

    pub fn empty_deployment_diff() -> diff::DeploymentDiff {
        diff::DeploymentDiff {
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
        }
    }

    pub fn unified_diffs(
        &self,
        masking: MaskingConfig,
        full_diff: bool,
    ) -> anyhow::Result<DeployUnifiedDiffs> {
        let mode = deployment_display_mode(full_diff);
        let local_agent_types = self
            .deployable_components
            .iter()
            .map(|(component_name, component)| {
                (component_name.0.clone(), component.agent_types.clone())
            })
            .collect::<HashMap<_, _>>();

        Ok(DeployUnifiedDiffs {
            display_diff_stage: self
                .diff_stage
                .as_ref()
                .map(|diff| {
                    let local = DeploymentDisplay::from_context(DeploymentDisplayContext {
                        masking,
                        mode,
                        deployment: &self.diffable_local_deployment,
                        diff,
                        agent_types_by_component: &local_agent_types,
                    })?;
                    let staged = DeploymentDisplay::from_context(DeploymentDisplayContext {
                        masking,
                        mode,
                        deployment: &self.diffable_staged_deployment,
                        diff,
                        agent_types_by_component: &self.staged_agent_types,
                    })?;
                    if full_diff {
                        local.unified_yaml_diff_with_current_full_context(&staged)
                    } else {
                        local.unified_yaml_diff_with_current(&staged)
                    }
                })
                .transpose()?,
            display_diff: {
                let local = DeploymentDisplay::from_context(DeploymentDisplayContext {
                    masking,
                    mode,
                    deployment: &self.diffable_local_deployment,
                    diff: &self.diff,
                    agent_types_by_component: &local_agent_types,
                })?;
                let current = DeploymentDisplay::from_context(DeploymentDisplayContext {
                    masking,
                    mode,
                    deployment: &self.diffable_current_deployment,
                    diff: &self.diff,
                    agent_types_by_component: &self.current_agent_types,
                })?;

                if full_diff {
                    local.unified_yaml_diff_with_current_full_context(&current)?
                } else {
                    local.unified_yaml_diff_with_current(&current)?
                }
            },
            environment_setup_report: self
                .environment_setup
                .as_ref()
                .and_then(|setup| (!setup.display.is_empty()).then_some(setup))
                .map(|setup| setup.display.to_yaml_report())
                .transpose()?,
        })
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

    pub fn deployable_manifest_mcp_deployment(
        &self,
        domain: &Domain,
    ) -> &McpDeploymentDeployProperties {
        self.deployable_mcp_deployments
            .get(domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected MCP deployment {} not found in deployable manifest",
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

    pub fn staged_mcp_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry {
        self.staged_deployment
            .mcp_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected MCP deployment {} not found in staged deployment",
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

    pub fn current_mcp_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry {
        self.current_deployment
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "Expected MCP deployment {} not found in deployment plan, no deployment",
                    domain
                )
            })
            .mcp_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected MCP deployment {} not found in current deployment",
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

    pub fn mcp_deployment_identity(
        &self,
        kind: DeployDiffKind,
        domain: &Domain,
    ) -> &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry {
        match kind {
            DeployDiffKind::Stage => self.staged_mcp_deployment_identity(domain),
            DeployDiffKind::Current => self.current_mcp_deployment_identity(domain),
        }
    }

    pub fn add_details(
        &mut self,
        kind: DeployDiffKind,
        details: DeployDetails,
    ) -> anyhow::Result<()> {
        for (component_name, component) in details.component {
            self.add_component_details(kind, component_name, component)?;
        }

        for (domain, http_api_deployment) in details.http_api_deployment {
            self.add_http_api_deployment_details(kind, domain, http_api_deployment)
        }

        for (domain, mcp_deployment) in details.mcp_deployment {
            self.add_mcp_deployment_details(kind, domain, mcp_deployment)
        }

        match kind {
            DeployDiffKind::Stage => {
                self.diff_stage = self
                    .diffable_staged_deployment
                    .diff_with_new(&self.diffable_local_deployment)?;
            }
            DeployDiffKind::Current => {
                self.diff = self
                    .diffable_current_deployment
                    .diff_with_new(&self.diffable_local_deployment)?
                    .unwrap_or_else(Self::empty_deployment_diff);
            }
        }

        Ok(())
    }

    fn add_component_details(
        &mut self,
        kind: DeployDiffKind,
        component_name: ComponentName,
        component: ComponentDto,
    ) -> anyhow::Result<()> {
        match kind {
            DeployDiffKind::Stage => {
                self.diffable_staged_deployment
                    .components
                    .insert(component_name.0.clone(), component.to_diffable()?.into());
                self.staged_agent_types
                    .insert(component_name.0, component.metadata.agent_types().to_vec());
            }
            DeployDiffKind::Current => {
                self.diffable_current_deployment
                    .components
                    .insert(component_name.0.clone(), component.to_diffable()?.into());
                self.current_agent_types
                    .insert(component_name.0, component.metadata.agent_types().to_vec());
            }
        }

        Ok(())
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

    fn add_mcp_deployment_details(
        &mut self,
        kind: DeployDiffKind,
        domain: Domain,
        mcp_deployment: McpDeployment,
    ) {
        match &kind {
            DeployDiffKind::Stage => {
                self.diffable_staged_deployment
                    .mcp_deployments
                    .insert(domain.0, mcp_deployment.to_diffable().into());
            }
            DeployDiffKind::Current => {
                self.diffable_current_deployment
                    .mcp_deployments
                    .insert(domain.0, mcp_deployment.to_diffable().into());
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
    pub mcp_deployment: Vec<(Domain, McpDeployment)>,
}

#[derive(Debug)]
pub struct DeployUnifiedDiffs {
    pub display_diff_stage: Option<String>,
    pub display_diff: String,
    pub environment_setup_report: Option<String>,
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
    pub current_agent_types: HashMap<String, Vec<AgentTypeSchema>>,
    pub target_agent_types: HashMap<String, Vec<AgentTypeSchema>>,
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

    pub fn current_mcp_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry {
        self.current_deployment
            .mcp_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected MCP deployment {} not found in current deployment",
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

    pub fn target_mcp_deployment_identity(
        &self,
        domain: &Domain,
    ) -> &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry {
        self.target_deployment
            .mcp_deployments
            .iter()
            .find(|dep| &dep.domain == domain)
            .unwrap_or_else(|| {
                panic!(
                    "Expected MCP deployment {} not found in target deployment",
                    domain
                )
            })
    }

    fn add_component_details(
        &mut self,
        component_details: RollbackEntityDetails<ComponentName, ComponentDto>,
    ) -> anyhow::Result<()> {
        if let Some(component) = component_details.new {
            self.diffable_target_deployment.components.insert(
                component_details.name.0.clone(),
                component.to_diffable()?.into(),
            );
            self.target_agent_types.insert(
                component_details.name.0.clone(),
                component.metadata.agent_types().to_vec(),
            );
        }
        if let Some(component) = component_details.current {
            self.diffable_current_deployment.components.insert(
                component_details.name.0.clone(),
                component.to_diffable()?.into(),
            );
            self.current_agent_types.insert(
                component_details.name.0.clone(),
                component.metadata.agent_types().to_vec(),
            );
        }

        Ok(())
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

    fn add_mcp_deployment_details(
        &mut self,
        mcp_deployment_details: RollbackEntityDetails<Domain, McpDeployment>,
    ) {
        if let Some(mcp_deployment) = mcp_deployment_details.new {
            self.diffable_target_deployment.mcp_deployments.insert(
                mcp_deployment_details.name.0.clone(),
                mcp_deployment.to_diffable().into(),
            );
        }
        if let Some(mcp_deployment) = mcp_deployment_details.current {
            self.diffable_current_deployment.mcp_deployments.insert(
                mcp_deployment_details.name.0.clone(),
                mcp_deployment.to_diffable().into(),
            );
        }
    }

    pub fn add_details(&mut self, details: RollbackDetails) -> anyhow::Result<()> {
        for component_details in details.component {
            self.add_component_details(component_details)?;
        }

        for http_api_deployment_details in details.http_api_deployment {
            self.add_http_api_deployment_details(http_api_deployment_details);
        }

        for mcp_deployment_details in details.mcp_deployment {
            self.add_mcp_deployment_details(mcp_deployment_details);
        }

        match self
            .diffable_target_deployment
            .diff_with_current(&self.diffable_current_deployment)?
        {
            Some(diff) => {
                self.diff = diff;
            }
            None => {
                bail!(
                    "Illegal state: empty diff between taget and current deployment after adding details"
                );
            }
        }

        Ok(())
    }

    pub fn unified_diffs(
        &self,
        masking: MaskingConfig,
        full_diff: bool,
    ) -> anyhow::Result<RollbackUnifiedDiffs> {
        let mode = deployment_display_mode(full_diff);
        Ok(RollbackUnifiedDiffs {
            display_diff: {
                let target = DeploymentDisplay::from_context(DeploymentDisplayContext {
                    masking,
                    mode,
                    deployment: &self.diffable_target_deployment,
                    diff: &self.diff,
                    agent_types_by_component: &self.target_agent_types,
                })?;
                let current = DeploymentDisplay::from_context(DeploymentDisplayContext {
                    masking,
                    mode,
                    deployment: &self.diffable_current_deployment,
                    diff: &self.diff,
                    agent_types_by_component: &self.current_agent_types,
                })?;

                if full_diff {
                    target.unified_yaml_diff_with_current_full_context(&current)?
                } else {
                    target.unified_yaml_diff_with_current(&current)?
                }
            },
        })
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
    pub mcp_deployment: Vec<RollbackEntityDetails<Domain, McpDeployment>>,
}

#[derive(Debug)]
pub struct RollbackUnifiedDiffs {
    pub display_diff: String,
}

fn deployment_display_mode(full_diff: bool) -> DeploymentDisplayMode {
    if full_diff {
        DeploymentDisplayMode::Full
    } else {
        DeploymentDisplayMode::ChangedOnly
    }
}
