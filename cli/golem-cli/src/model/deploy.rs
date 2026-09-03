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

use crate::agent_id_display::{SourceLanguage, render_type_for_language};
use crate::command::shared_args::{ForceBuildArg, PostDeployArgs};
use crate::error::service::ServiceError;
use crate::log::{LogColorize, logln};
use crate::model::agent::RawAgentId;
use crate::model::cli_output::StructuredOutput;
use crate::model::component::{
    render_agent_constructor, render_input_schema, render_output_schema,
};
use crate::model::language::GuestLanguage;
use crate::model::masking::{
    Masked, MaskingConfig, is_sensitive_key, mask_json_secret_for_deploy_diff,
    mask_secret_with_fingerprint,
};
use crate::model::text_format::{
    Column, FieldsBuilder, MessageWithFields, NoTextOutput, TextOutput, format_id, format_main_id,
    log_table, new_table_full_condensed,
};
use colored::Colorize;
use golem_client::model::{AgentSecretDto, Deployment, RetryPolicyDto};
use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::agent::{
    AgentConfigSource, HttpEndpointDetails, HttpMethod, HttpMountDetails, PathSegment,
};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::PolymorphicPermissionPattern;
use golem_common::model::component::{AgentFilePermissions, ComponentName, ComponentRevision};
use golem_common::model::deployment::{
    CurrentDeployment, DeploymentAgentSecretDefault, DeploymentRetryPolicyDefault,
};
use golem_common::model::diff::{
    self, AgentTypeProvisionConfigDiff, BTreeMapDiffValue, DeploymentDiff, DiffForHashOf, Hashable,
};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::environment_tool_grant::EnvironmentToolGrantId;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionCreation};
use golem_common::model::tool_release::ToolReleaseId;
use golem_common::schema::agent::{AgentMethodSchema, AgentTypeSchema};
use golem_common::schema::graph::SchemaGraph;
use itertools::Itertools;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplay {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, DeploymentDisplayComponent>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub remote_tools: BTreeMap<String, DeploymentDisplayRemoteTool>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub published_tools: BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub http_api_deployments: BTreeMap<String, DeploymentDisplayHttpApiDeployment>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_deployments: BTreeMap<String, DeploymentDisplayMcpDeployment>,
}

pub struct DeploymentDisplayContext<'a> {
    pub masking: MaskingConfig,
    pub mode: DeploymentDisplayMode,
    pub deployment: &'a diff::Deployment,
    pub diff: &'a diff::DeploymentDiff,
    pub agent_types_by_component: &'a HashMap<String, Vec<AgentTypeSchema>>,
}

#[derive(Clone, Copy, Debug)]
pub enum DeploymentDisplayMode {
    ChangedOnly,
    Full,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupDisplay {
    #[serde(skip_serializing_if = "EnvironmentSetupDetailedSection::is_empty")]
    #[serde(default)]
    pub to_be_applied: EnvironmentSetupDetailedSection,
    #[serde(skip_serializing_if = "EnvironmentSetupKeysOnlySection::is_empty")]
    #[serde(default)]
    pub skipped_already_exists: EnvironmentSetupKeysOnlySection,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupDetailedSection {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(default)]
    pub secret_values: BTreeMap<String, EnvironmentSetupSecretValueDisplay>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(default)]
    pub retry_policies: BTreeMap<String, EnvironmentSetupRetryPolicyDisplay>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(default)]
    pub resources: BTreeMap<String, EnvironmentSetupResourceDisplay>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupKeysOnlySection {
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    #[serde(default)]
    pub secret_values: BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    #[serde(default)]
    pub retry_policies: BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    #[serde(default)]
    pub resources: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupSecretValueDisplay {
    pub secret_type: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupRetryPolicyDisplay {
    pub priority: u32,
    pub predicate: serde_json::Value,
    pub policy: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupResourceDisplay {
    pub limit: serde_json::Value,
    pub enforcement_action: String,
    pub unit: String,
    pub units: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSetupPlan {
    pub display: EnvironmentSetupDisplay,
    pub agent_secret_defaults: Vec<DeploymentAgentSecretDefault>,
    pub skipped_existing_agent_secret_defaults: Vec<DeploymentAgentSecretDefault>,
    pub retry_policy_defaults: Vec<DeploymentRetryPolicyDefault>,
    pub resource_defaults: Vec<ResourceDefinitionCreation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnvironmentToolGrantPlanAction {
    Create,
    Delete,
    RetainProtected,
    RetainAdministratorManaged,
}

impl std::fmt::Display for EnvironmentToolGrantPlanAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Create => "create",
            Self::Delete => "delete",
            Self::RetainProtected => "retain protected",
            Self::RetainAdministratorManaged => "retain administrator-managed",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantPlanEntry {
    pub action: EnvironmentToolGrantPlanAction,
    pub release_id: Option<ToolReleaseId>,
    pub account: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub grant_id: Option<EnvironmentToolGrantId>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolGrantPlanView {
    pub entries: Vec<EnvironmentToolGrantPlanEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolPublicationPlanAction {
    NoChange,
    Publish,
    Conflict,
}

impl std::fmt::Display for ToolPublicationPlanAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoChange => "no change",
            Self::Publish => "publish",
            Self::Conflict => "conflict",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPublicationPlanEntry {
    pub action: ToolPublicationPlanAction,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPublicationPlan {
    pub entries: Vec<ToolPublicationPlanEntry>,
}

impl ToolPublicationPlan {
    pub fn has_work(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry.action, ToolPublicationPlanAction::Publish))
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.action == ToolPublicationPlanAction::Conflict)
    }
}

impl EnvironmentToolGrantPlanView {
    pub fn has_changes(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry.action,
                EnvironmentToolGrantPlanAction::Create | EnvironmentToolGrantPlanAction::Delete
            )
        })
    }
}

impl StructuredOutput for EnvironmentToolGrantPlanView {
    const KIND: &'static str = "deploy.environment-tool-grants";
}

impl TextOutput for EnvironmentToolGrantPlanView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Action"),
            Column::new("Release ID"),
            Column::new("Account"),
            Column::new("Tool"),
            Column::new("Version"),
            Column::new("Grant ID"),
        ]);
        for entry in &self.entries {
            table.add_row(vec![
                entry.action.to_string(),
                entry
                    .release_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                entry.account.clone().unwrap_or_default(),
                entry.name.clone().unwrap_or_default(),
                entry.version.clone().unwrap_or_default(),
                entry.grant_id.map(|id| id.to_string()).unwrap_or_default(),
            ]);
        }
        log_table(table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::UntypedJsonBody;
    use golem_common::base_model::retry_policy::{ApiNeverPolicy, ApiPredicateFalse};
    use golem_common::base_model::retry_policy::{ApiPredicate, ApiRetryPolicy};
    use golem_common::model::agent_secret::{AgentSecretId, AgentSecretPath};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::quota::{
        EnforcementAction, ResourceCapacityLimit, ResourceDefinitionId, ResourceLimit, ResourceName,
    };
    use golem_common::model::retry_policy::{RetryPolicyId, RetryPolicyRevision};
    use golem_common::schema::schema_type::SchemaType;
    use golem_common::schema::{SchemaGraph, SchemaValue};
    use uuid::Uuid;

    fn schema_str() -> SchemaType {
        SchemaType::string()
    }

    fn secret_dto(
        path: &[&str],
        secret_type: SchemaGraph,
        value: Option<SchemaValue>,
    ) -> AgentSecretDto {
        AgentSecretDto {
            id: AgentSecretId(Uuid::nil()),
            environment_id: EnvironmentId(Uuid::nil()),
            path: CanonicalAgentSecretPath::from_path_in_unknown_casing(
                &path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ),
            revision: serde_json::from_value(serde_json::json!(0)).unwrap(),
            secret_type,
            secret_value: value,
        }
    }

    fn retry_policy(name: &str, priority: u32) -> RetryPolicyDto {
        RetryPolicyDto {
            id: RetryPolicyId(Uuid::nil()),
            environment_id: EnvironmentId(Uuid::nil()),
            name: name.to_string(),
            revision: RetryPolicyRevision::INITIAL,
            priority,
            predicate: UntypedJsonBody(
                serde_json::to_value(ApiPredicate::False(ApiPredicateFalse {})).unwrap(),
            ),
            policy: UntypedJsonBody(
                serde_json::to_value(ApiRetryPolicy::Never(ApiNeverPolicy {})).unwrap(),
            ),
        }
    }

    fn resource(name: &str, limit_value: u64) -> ResourceDefinition {
        ResourceDefinition {
            id: ResourceDefinitionId(Uuid::nil()),
            revision: serde_json::from_value(serde_json::json!(0)).unwrap(),
            environment_id: EnvironmentId(Uuid::nil()),
            name: ResourceName(name.to_string()),
            limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: limit_value }),
            enforcement_action: EnforcementAction::Reject,
            unit: "unit".to_string(),
            units: "units".to_string(),
        }
    }

    fn resource_creation(name: &str, limit_value: u64) -> ResourceDefinitionCreation {
        ResourceDefinitionCreation {
            name: ResourceName(name.to_string()),
            limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: limit_value }),
            enforcement_action: EnforcementAction::Reject,
            unit: "unit".to_string(),
            units: "units".to_string(),
        }
    }

    #[::test_r::test]
    fn tool_publication_plan_distinguishes_work_and_conflicts() {
        let entry = |action| ToolPublicationPlanEntry {
            action,
            name: "example".to_string(),
            version: "1.0.0".to_string(),
            reason: None,
        };

        assert!(
            !ToolPublicationPlan {
                entries: vec![entry(ToolPublicationPlanAction::NoChange)]
            }
            .has_work()
        );
        assert!(
            ToolPublicationPlan {
                entries: vec![entry(ToolPublicationPlanAction::Publish)]
            }
            .has_work()
        );
        assert!(
            ToolPublicationPlan {
                entries: vec![entry(ToolPublicationPlanAction::Conflict)]
            }
            .has_conflicts()
        );
    }

    #[::test_r::test]
    fn environment_setup_secret_type_rendering_matches_between_manifest_and_environment() {
        let mut secret_types = BTreeMap::new();
        secret_types.insert("superSecret".to_string(), schema_str());

        let plan = build_environment_setup_plan(
            MaskingConfig::hide_secrets(),
            vec![DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec!["superSecret".to_string()]),
                secret_value: serde_json::json!("same-value"),
            }],
            Vec::new(),
            Vec::new(),
            vec![secret_dto(
                &["superSecret"],
                SchemaGraph::anonymous(SchemaType::string()),
                Some(SchemaValue::String("same-value".to_string())),
            )],
            Vec::new(),
            Vec::new(),
            &secret_types,
            &SourceLanguage::TypeScript,
        )
        .unwrap();

        assert!(
            plan.display
                .skipped_already_exists
                .secret_values
                .contains("superSecret")
        );
    }

    #[::test_r::test]
    fn environment_setup_classifies_secret_create_and_skip_existing() {
        let mut secret_types = BTreeMap::new();
        secret_types.insert("createSecret".to_string(), schema_str());
        secret_types.insert("existingSecret".to_string(), schema_str());

        let plan = build_environment_setup_plan(
            MaskingConfig::hide_secrets(),
            vec![
                DeploymentAgentSecretDefault {
                    path: AgentSecretPath(vec!["createSecret".to_string()]),
                    secret_value: serde_json::json!("create"),
                },
                DeploymentAgentSecretDefault {
                    path: AgentSecretPath(vec!["existingSecret".to_string()]),
                    secret_value: serde_json::json!("manifest"),
                },
            ],
            Vec::new(),
            Vec::new(),
            vec![secret_dto(
                &["existingSecret"],
                SchemaGraph::anonymous(SchemaType::string()),
                Some(SchemaValue::String("env".to_string())),
            )],
            Vec::new(),
            Vec::new(),
            &secret_types,
            &SourceLanguage::TypeScript,
        )
        .unwrap();

        assert!(
            plan.display
                .to_be_applied
                .secret_values
                .contains_key("createSecret")
        );
        assert!(
            plan.display
                .skipped_already_exists
                .secret_values
                .contains("existingSecret")
        );
    }

    #[::test_r::test]
    fn environment_setup_classifies_retry_policies_and_resources() {
        let plan = build_environment_setup_plan(
            MaskingConfig::hide_secrets(),
            Vec::new(),
            vec![
                DeploymentRetryPolicyDefault {
                    name: "create-policy".to_string(),
                    priority: 1,
                    predicate: ApiPredicate::False(ApiPredicateFalse {}),
                    policy: ApiRetryPolicy::Never(ApiNeverPolicy {}),
                },
                DeploymentRetryPolicyDefault {
                    name: "existing-policy".to_string(),
                    priority: 2,
                    predicate: ApiPredicate::False(ApiPredicateFalse {}),
                    policy: ApiRetryPolicy::Never(ApiNeverPolicy {}),
                },
            ],
            vec![
                resource_creation("create-resource", 1),
                resource_creation("existing-resource", 2),
            ],
            Vec::new(),
            vec![retry_policy("existing-policy", 999)],
            vec![resource("existing-resource", 999)],
            &BTreeMap::new(),
            &SourceLanguage::TypeScript,
        )
        .unwrap();

        assert!(
            plan.display
                .to_be_applied
                .retry_policies
                .contains_key("create-policy")
        );
        assert!(
            plan.display
                .skipped_already_exists
                .retry_policies
                .contains("existing-policy")
        );

        assert!(
            plan.display
                .to_be_applied
                .resources
                .contains_key("create-resource")
        );
        assert!(
            plan.display
                .skipped_already_exists
                .resources
                .contains("existing-resource")
        );
    }
}

impl EnvironmentSetupDetailedSection {
    pub fn is_empty(&self) -> bool {
        self.secret_values.is_empty() && self.retry_policies.is_empty() && self.resources.is_empty()
    }
}

impl EnvironmentSetupKeysOnlySection {
    pub fn is_empty(&self) -> bool {
        self.secret_values.is_empty() && self.retry_policies.is_empty() && self.resources.is_empty()
    }
}

impl EnvironmentSetupDisplay {
    pub fn is_empty(&self) -> bool {
        self.to_be_applied.is_empty() && self.skipped_already_exists.is_empty()
    }

    pub fn has_entries_to_apply(&self) -> bool {
        !self.to_be_applied.is_empty()
    }

    pub fn has_entries_skipped_already_exists(&self) -> bool {
        !self.skipped_already_exists.is_empty()
    }

    pub fn to_yaml_report(&self) -> anyhow::Result<String> {
        if self.is_empty() {
            Ok(String::new())
        } else {
            Ok(serde_yaml::to_string(self)?)
        }
    }
}

pub fn preferred_source_language_for_setup(
    agent_types_by_component: &HashMap<String, Vec<AgentTypeSchema>>,
) -> SourceLanguage {
    let mut languages = agent_types_by_component
        .values()
        .flat_map(|agent_types| agent_types.iter())
        .filter_map(|agent_type| GuestLanguage::from_string(&agent_type.source_language))
        .collect::<Vec<_>>();

    languages.sort();
    languages.dedup();

    let selected = languages.into_iter().next();

    match selected {
        Some(GuestLanguage::Rust) => SourceLanguage::Rust,
        Some(GuestLanguage::TypeScript) => SourceLanguage::TypeScript,
        Some(GuestLanguage::Scala) => SourceLanguage::Scala,
        Some(GuestLanguage::MoonBit) => SourceLanguage::MoonBit,
        None => SourceLanguage::Other(String::new()),
    }
}

pub fn build_environment_setup_plan(
    masking: MaskingConfig,
    resolved_agent_secret_defaults: Vec<DeploymentAgentSecretDefault>,
    retry_policy_defaults: Vec<DeploymentRetryPolicyDefault>,
    resource_defaults: Vec<ResourceDefinitionCreation>,
    current_agent_secrets: Vec<AgentSecretDto>,
    current_retry_policies: Vec<RetryPolicyDto>,
    current_resources: Vec<ResourceDefinition>,
    secret_types_by_path: &BTreeMap<String, golem_common::schema::schema_type::SchemaType>,
    source_language: &SourceLanguage,
) -> anyhow::Result<EnvironmentSetupPlan> {
    let mut display = EnvironmentSetupDisplay::default();

    let local_secret_defaults = resolved_agent_secret_defaults
        .iter()
        .map(|default| {
            let canonical_path = CanonicalAgentSecretPath::from(default.path.clone());
            let canonical_path_str = canonical_path.to_string();
            Ok((
                canonical_path_str.clone(),
                EnvironmentSetupSecretValueDisplay {
                    secret_type: secret_types_by_path
                        .get(&canonical_path_str)
                        .map(|typ| render_schema_type_for_language(source_language, typ))
                        .unwrap_or_else(|| "unknown".to_string()),
                    value: mask_json_secret_for_deploy_diff(masking, &default.secret_value)?,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let current_secret_values = current_agent_secrets
        .into_iter()
        .map(|secret| {
            let value = match secret.secret_value {
                Some(value) => mask_json_secret_for_deploy_diff(masking, &value)?,
                None => serde_json::Value::Null,
            };
            Ok((
                secret.path.to_string(),
                EnvironmentSetupSecretValueDisplay {
                    secret_type: render_type_for_language(
                        source_language,
                        &secret.secret_type,
                        &secret.secret_type.root,
                        true,
                    ),
                    value,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let secret_defaults_by_path = resolved_agent_secret_defaults
        .iter()
        .map(|default| {
            (
                CanonicalAgentSecretPath::from(default.path.clone()).to_string(),
                default,
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut to_be_applied_agent_secret_defaults = Vec::new();
    let mut skipped_existing_agent_secret_defaults = Vec::new();

    classify_environment_setup_entries(
        &mut display,
        local_secret_defaults,
        current_secret_values,
        |section, key, value| {
            section.secret_values.insert(key.clone(), value);
            if let Some(default) = secret_defaults_by_path.get(&key) {
                to_be_applied_agent_secret_defaults.push((*default).clone());
            }
        },
        |section, key| {
            section.secret_values.insert(key.clone());
            if let Some(default) = secret_defaults_by_path.get(&key) {
                skipped_existing_agent_secret_defaults.push((*default).clone());
            }
        },
    );

    let local_retry_policy_defaults = retry_policy_defaults
        .iter()
        .map(|policy| {
            Ok((
                policy.name.clone(),
                EnvironmentSetupRetryPolicyDisplay {
                    priority: policy.priority,
                    predicate: serde_json::to_value(&policy.predicate)?,
                    policy: serde_json::to_value(&policy.policy)?,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let current_retry_policy_values = current_retry_policies
        .into_iter()
        .map(|policy| {
            Ok((
                policy.name.clone(),
                EnvironmentSetupRetryPolicyDisplay {
                    priority: policy.priority,
                    predicate: serde_json::to_value(&policy.predicate)?,
                    policy: serde_json::to_value(&policy.policy)?,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    classify_environment_setup_entries(
        &mut display,
        local_retry_policy_defaults,
        current_retry_policy_values,
        |section, key, value| {
            section.retry_policies.insert(key, value);
        },
        |section, key| {
            section.retry_policies.insert(key);
        },
    );

    let local_resource_defaults = resource_defaults
        .iter()
        .map(|resource| {
            Ok((
                resource.name.0.clone(),
                EnvironmentSetupResourceDisplay {
                    limit: serde_json::to_value(&resource.limit)?,
                    enforcement_action: resource.enforcement_action.to_string(),
                    unit: resource.unit.clone(),
                    units: resource.units.clone(),
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let current_resource_values = current_resources
        .into_iter()
        .map(|resource| {
            Ok((
                resource.name.0.clone(),
                EnvironmentSetupResourceDisplay {
                    limit: serde_json::to_value(&resource.limit)?,
                    enforcement_action: resource.enforcement_action.to_string(),
                    unit: resource.unit,
                    units: resource.units,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    classify_environment_setup_entries(
        &mut display,
        local_resource_defaults,
        current_resource_values,
        |section, key, value| {
            section.resources.insert(key, value);
        },
        |section, key| {
            section.resources.insert(key);
        },
    );

    Ok(EnvironmentSetupPlan {
        display,
        agent_secret_defaults: to_be_applied_agent_secret_defaults,
        skipped_existing_agent_secret_defaults,
        retry_policy_defaults,
        resource_defaults,
    })
}

fn classify_environment_setup_entries<T: Clone + PartialEq>(
    display: &mut EnvironmentSetupDisplay,
    local: BTreeMap<String, T>,
    current: BTreeMap<String, T>,
    mut insert: impl FnMut(&mut EnvironmentSetupDetailedSection, String, T),
    mut insert_existing: impl FnMut(&mut EnvironmentSetupKeysOnlySection, String),
) {
    for (key, local_value) in &local {
        match current.get(key) {
            None => insert(&mut display.to_be_applied, key.clone(), local_value.clone()),
            Some(_) => insert_existing(&mut display.skipped_already_exists, key.clone()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayComponent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_hash: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, DeploymentDisplayAgentType>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayAgentType {
    pub constructor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source_language: String,
    pub mode: String,
    pub snapshotting: serde_json::Value,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config_declarations: BTreeMap<String, DeploymentDisplayConfigDeclaration>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config_defaults: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, DeploymentDisplayAgentFile>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, DeploymentDisplayPlugin>,
    #[serde(skip_serializing_if = "DeploymentDisplayInitialPermissions::is_empty")]
    pub initial_permissions: DeploymentDisplayInitialPermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_mount: Option<DeploymentDisplayHttpMount>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub methods: BTreeMap<String, DeploymentDisplayMethod>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayConfigDeclaration {
    pub source: String,
    pub value_type: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayAgentFile {
    pub permissions: String,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayPlugin {
    pub priority: i32,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayInitialPermissions {
    #[serde(skip_serializing_if = "DeploymentDisplayInitialPermissionsBound::is_empty")]
    pub lower_bound: DeploymentDisplayInitialPermissionsBound,
    #[serde(skip_serializing_if = "DeploymentDisplayInitialPermissionsBound::is_empty")]
    pub upper_bound: DeploymentDisplayInitialPermissionsBound,
}

impl DeploymentDisplayInitialPermissions {
    fn is_empty(&self) -> bool {
        self.lower_bound.is_empty() && self.upper_bound.is_empty()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayInitialPermissionsBound {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub positive: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub negative: Vec<String>,
}

impl DeploymentDisplayInitialPermissionsBound {
    fn is_empty(&self) -> bool {
        self.positive.is_empty() && self.negative.is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpMount {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    pub phantom_agent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayMethod {
    pub signature: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub http: Vec<DeploymentDisplayHttpEndpoint>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpEndpoint {
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpApiDeployment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhooks_prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub openapi_endpoint_prefix: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, DeploymentDisplayHttpApiAgentOptions>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpApiAgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_session_header: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayMcpDeployment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, DeploymentDisplayMcpAgentOptions>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayMcpAgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayRemoteTool {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provision: Option<golem_common::model::tool::ToolProvisionConfig>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, diff::EffectiveToolBinding>,
}

impl DeploymentDisplay {
    pub fn from_context(ctx: DeploymentDisplayContext<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            components: display_components(&ctx)?,
            remote_tools: display_remote_tools(&ctx)?,
            published_tools: display_published_tools(&ctx),
            http_api_deployments: display_http_api_deployments(&ctx),
            mcp_deployments: display_mcp_deployments(&ctx),
        })
    }

    pub fn unified_yaml_diff_with_current(&self, current: &Self) -> anyhow::Result<String> {
        Ok(diff::unified_diff(
            current.to_yaml_for_diff()?,
            self.to_yaml_for_diff()?,
        ))
    }

    pub fn unified_yaml_diff_with_current_full_context(
        &self,
        current: &Self,
    ) -> anyhow::Result<String> {
        Ok(diff::unified_diff_with_context(
            current.to_yaml_for_diff()?,
            self.to_yaml_for_diff()?,
            usize::MAX,
        ))
    }

    fn to_yaml_for_diff(&self) -> anyhow::Result<String> {
        if self.is_empty() {
            Ok(String::new())
        } else {
            Ok(serde_yaml::to_string(self)?)
        }
    }

    fn is_empty(&self) -> bool {
        self.components.is_empty()
            && self.remote_tools.is_empty()
            && self.published_tools.is_empty()
            && self.http_api_deployments.is_empty()
            && self.mcp_deployments.is_empty()
    }
}

fn display_remote_tools(
    ctx: &DeploymentDisplayContext<'_>,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayRemoteTool>> {
    display_keys(
        ctx.mode,
        &ctx.deployment.remote_tools,
        &ctx.diff.remote_tools,
    )
    .filter(|tool_name| ctx.deployment.remote_tools.contains_key(*tool_name))
    .map(|tool_name| {
        let remote_tool = ctx
            .deployment
            .remote_tools
            .get(tool_name)
            .expect("displayed remote tool must exist in deployment");
        let hash = Some(remote_tool.hash()?.to_string());
        let Some(remote_tool) = remote_tool.as_value() else {
            return Ok((
                tool_name.clone(),
                DeploymentDisplayRemoteTool {
                    hash,
                    release_id: None,
                    version: None,
                    source_digest: None,
                    owner_account: None,
                    metadata_version: None,
                    metadata_digest: None,
                    provision: None,
                    bindings: BTreeMap::new(),
                },
            ));
        };

        let mut provision = remote_tool.provision.clone();
        provision.config = NormalizedJsonValue(mask_json_secret_for_deploy_diff(
            ctx.masking,
            &provision.config,
        )?);
        provision.env = display_env(ctx.masking, &provision.env);
        for plugin in &mut provision.plugins {
            plugin.parameters = plugin
                .parameters
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        mask_sensitive_key_value_for_deploy_diff(ctx.masking, key, value),
                    )
                })
                .collect();
        }

        let bindings = remote_tool
            .bindings
            .iter()
            .map(|(agent, binding)| {
                let mut binding = binding.clone();
                binding.parameters = NormalizedJsonValue(mask_json_secret_for_deploy_diff(
                    ctx.masking,
                    &binding.parameters,
                )?);
                Ok((agent.to_string(), binding))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

        Ok((
            tool_name.clone(),
            DeploymentDisplayRemoteTool {
                hash,
                release_id: Some(remote_tool.release_id.to_string()),
                version: Some(remote_tool.version.clone()),
                source_digest: Some(remote_tool.source_digest.to_string()),
                owner_account: Some(remote_tool.owner_account_email.to_string()),
                metadata_version: Some(remote_tool.metadata_version.clone()),
                metadata_digest: Some(remote_tool.metadata_digest.to_string()),
                provision: Some(provision),
                bindings,
            },
        ))
    })
    .collect()
}

fn display_published_tools(ctx: &DeploymentDisplayContext<'_>) -> BTreeSet<String> {
    match ctx.mode {
        DeploymentDisplayMode::ChangedOnly => ctx
            .diff
            .published_tools
            .keys()
            .filter(|tool_name| ctx.deployment.published_tools.contains(*tool_name))
            .cloned()
            .collect(),
        DeploymentDisplayMode::Full => ctx.deployment.published_tools.clone(),
    }
}

fn display_components(
    ctx: &DeploymentDisplayContext<'_>,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayComponent>> {
    display_keys(ctx.mode, &ctx.deployment.components, &ctx.diff.components)
        .filter(|component_name| ctx.deployment.components.contains_key(*component_name))
        .map(|component_name| {
            let agent_types = ctx
                .agent_types_by_component
                .get(component_name)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let component = ctx
                .deployment
                .components
                .get(component_name)
                .and_then(|component| component.as_value());

            let binary_hash = component.map(|component| component.wasm_hash.to_string());

            let agents = agent_types
                .iter()
                .sorted_by_key(|agent| &agent.type_name.0)
                .map(|agent| {
                    Ok((
                        agent.type_name.0.clone(),
                        display_agent_type(ctx.masking, agent, component)?,
                    ))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

            Ok((
                component_name.clone(),
                DeploymentDisplayComponent {
                    binary_hash,
                    agents,
                },
            ))
        })
        .collect()
}

fn display_agent_type(
    masking: MaskingConfig,
    agent: &AgentTypeSchema,
    component: Option<&diff::Component>,
) -> anyhow::Result<DeploymentDisplayAgentType> {
    let lang = SourceLanguage::from(agent.source_language.as_str());
    let provision_config = component
        .and_then(|component| {
            component
                .agent_type_provision_configs
                .get(&agent.type_name.0)
        })
        .and_then(|config| config.as_value());

    Ok(DeploymentDisplayAgentType {
        constructor: render_agent_constructor(agent, false, false),
        description: agent.description.clone(),
        source_language: agent.source_language.clone(),
        mode: agent.mode.to_string(),
        snapshotting: serde_json::to_value(&agent.snapshotting)?,
        config_declarations: display_config_declarations(agent)?,
        config_defaults: display_config_defaults(masking, agent, provision_config)?,
        env: provision_config
            .map(|config| display_env(masking, &config.env))
            .unwrap_or_default(),
        files: provision_config
            .map(display_files)
            .transpose()?
            .unwrap_or_default(),
        plugins: provision_config
            .map(|config| display_plugins(masking, config))
            .unwrap_or_default(),
        initial_permissions: provision_config
            .map(display_initial_permission)
            .transpose()?
            .unwrap_or_default(),
        http_mount: agent.http_mount.as_ref().map(display_http_mount),
        methods: agent
            .methods
            .iter()
            .map(|method| {
                (
                    method.name.clone(),
                    display_method(&lang, &agent.schema, method),
                )
            })
            .collect(),
        dependencies: agent
            .dependencies
            .iter()
            .map(|dependency| dependency.type_name.clone())
            .collect(),
    })
}

fn display_config_declarations(
    agent: &AgentTypeSchema,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayConfigDeclaration>> {
    let lang = SourceLanguage::from(agent.source_language.as_str());

    agent
        .config
        .iter()
        .map(|config| {
            let value_type =
                render_type_for_language(&lang, &agent.schema, &config.value_type, true);
            Ok((
                config.path.join("."),
                DeploymentDisplayConfigDeclaration {
                    source: render_agent_config_source(config.source).to_string(),
                    value_type,
                },
            ))
        })
        .collect()
}

fn display_config_defaults(
    masking: MaskingConfig,
    agent: &AgentTypeSchema,
    provision_config: Option<&diff::AgentTypeProvisionConfig>,
) -> anyhow::Result<BTreeMap<String, serde_json::Value>> {
    let provision_values = provision_config
        .map(|config| &config.config)
        .into_iter()
        .flatten()
        .collect::<BTreeMap<_, _>>();

    let mut result = BTreeMap::new();

    for (path, value) in &provision_values {
        let declaration = agent
            .config
            .iter()
            .find(|config| config.path.join(".") == path.as_str());
        let is_secret =
            declaration.is_some_and(|config| config.source == AgentConfigSource::Secret);

        let rendered_value = if is_secret && !masking.show_secrets {
            mask_json_secret_for_deploy_diff(MaskingConfig::hide_secrets(), value)?
        } else {
            serde_json::to_value(value)?
        };

        result.insert((*path).clone(), rendered_value);
    }

    Ok(result)
}

fn display_env(masking: MaskingConfig, env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                mask_sensitive_key_value_for_deploy_diff(masking, key, value),
            )
        })
        .collect()
}

fn display_files(
    provision_config: &diff::AgentTypeProvisionConfig,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayAgentFile>> {
    provision_config
        .files_by_path
        .iter()
        .filter_map(|(path, file)| file.as_value().map(|file| (path, file)))
        .map(|(path, file)| {
            Ok((
                path.clone(),
                DeploymentDisplayAgentFile {
                    permissions: display_agent_file_permissions(file.permissions).to_string(),
                    hash: file.hash.to_string(),
                },
            ))
        })
        .collect()
}

fn display_plugins(
    masking: MaskingConfig,
    provision_config: &diff::AgentTypeProvisionConfig,
) -> BTreeMap<String, DeploymentDisplayPlugin> {
    provision_config
        .plugins_by_grant_id
        .values()
        .map(|plugin| {
            let key = format!("{}@{}", plugin.name, plugin.version);
            let parameters = plugin
                .parameters
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        mask_sensitive_key_value_for_deploy_diff(masking, key, value),
                    )
                })
                .collect();

            (
                key,
                DeploymentDisplayPlugin {
                    priority: plugin.priority,
                    parameters,
                },
            )
        })
        .collect()
}

fn display_initial_permission(
    provision_config: &diff::AgentTypeProvisionConfig,
) -> anyhow::Result<DeploymentDisplayInitialPermissions> {
    Ok(DeploymentDisplayInitialPermissions {
        lower_bound: DeploymentDisplayInitialPermissionsBound {
            positive: render_permissions(&provision_config.initial_permissions.lower_positive)?,
            negative: render_permissions(&provision_config.initial_permissions.lower_negative)?,
        },
        upper_bound: DeploymentDisplayInitialPermissionsBound {
            positive: render_permissions(&provision_config.initial_permissions.upper_positive)?,
            negative: render_permissions(&provision_config.initial_permissions.upper_negative)?,
        },
    })
}

fn render_permissions(permissions: &[PolymorphicPermissionPattern]) -> anyhow::Result<Vec<String>> {
    permissions
        .iter()
        .map(|p| p.render())
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::msg)
}

fn display_method(
    lang: &SourceLanguage,
    graph: &SchemaGraph,
    method: &AgentMethodSchema,
) -> DeploymentDisplayMethod {
    let output = render_output_schema(graph, &method.output_schema, lang);
    let input = render_input_schema(graph, &method.input_schema, lang, true);
    let signature = if output.is_empty() {
        format!("{}({})", method.name, input)
    } else {
        format!("{}({}) -> {}", method.name, input, output)
    };

    DeploymentDisplayMethod {
        signature,
        description: method.description.clone(),
        prompt_hint: method.prompt_hint.clone(),
        http: method
            .http_endpoint
            .iter()
            .map(display_http_endpoint)
            .collect(),
    }
}

fn display_http_mount(http_mount: &HttpMountDetails) -> DeploymentDisplayHttpMount {
    DeploymentDisplayHttpMount {
        path: render_path(&http_mount.path_prefix),
        webhook: (!http_mount.webhook_suffix.is_empty())
            .then(|| render_path(&http_mount.webhook_suffix)),
        phantom_agent: http_mount.phantom_agent,
        auth_required: http_mount.auth_details.as_ref().map(|auth| auth.required),
        cors: http_mount.cors_options.allowed_patterns.clone(),
    }
}

fn display_http_endpoint(endpoint: &HttpEndpointDetails) -> DeploymentDisplayHttpEndpoint {
    DeploymentDisplayHttpEndpoint {
        method: render_http_method(&endpoint.http_method).to_string(),
        path: render_path(&endpoint.path_suffix),
        auth_required: endpoint.auth_details.as_ref().map(|auth| auth.required),
        headers: endpoint
            .header_vars
            .iter()
            .map(|header| (header.header_name.clone(), header.variable_name.clone()))
            .collect(),
        query: endpoint
            .query_vars
            .iter()
            .map(|query| (query.query_param_name.clone(), query.variable_name.clone()))
            .collect(),
        cors: endpoint.cors_options.allowed_patterns.clone(),
    }
}

fn display_http_api_deployments(
    ctx: &DeploymentDisplayContext<'_>,
) -> BTreeMap<String, DeploymentDisplayHttpApiDeployment> {
    display_keys(
        ctx.mode,
        &ctx.deployment.http_api_deployments,
        &ctx.diff.http_api_deployments,
    )
    .filter_map(|domain| {
        let hash = ctx
            .deployment
            .http_api_deployments
            .get(domain)
            .and_then(|deployment| deployment.hash().ok())
            .map(|hash| hash.to_string());
        ctx.deployment
            .http_api_deployments
            .get(domain)
            .and_then(|deployment| deployment.as_value())
            .map(|deployment| {
                (
                    domain.clone(),
                    DeploymentDisplayHttpApiDeployment {
                        hash: hash.clone(),
                        webhooks_prefix: deployment.webhooks_prefix.clone(),
                        openapi_endpoint_prefix: deployment.openapi_endpoint_prefix.clone(),
                        agents: deployment
                            .agents
                            .iter()
                            .map(|(agent, options)| {
                                (
                                    agent.clone(),
                                    DeploymentDisplayHttpApiAgentOptions {
                                        security_scheme: options.security_scheme.clone(),
                                        test_session_header: options.test_session_header.clone(),
                                    },
                                )
                            })
                            .collect(),
                    },
                )
            })
            .or_else(|| {
                hash.map(|hash| {
                    (
                        domain.clone(),
                        DeploymentDisplayHttpApiDeployment {
                            hash: Some(hash),
                            webhooks_prefix: String::new(),
                            openapi_endpoint_prefix: String::new(),
                            agents: BTreeMap::new(),
                        },
                    )
                })
            })
    })
    .collect()
}

fn display_mcp_deployments(
    ctx: &DeploymentDisplayContext<'_>,
) -> BTreeMap<String, DeploymentDisplayMcpDeployment> {
    display_keys(
        ctx.mode,
        &ctx.deployment.mcp_deployments,
        &ctx.diff.mcp_deployments,
    )
    .filter_map(|domain| {
        let hash = ctx
            .deployment
            .mcp_deployments
            .get(domain)
            .and_then(|deployment| deployment.hash().ok())
            .map(|hash| hash.to_string());
        ctx.deployment
            .mcp_deployments
            .get(domain)
            .and_then(|deployment| deployment.as_value())
            .map(|deployment| {
                (
                    domain.clone(),
                    DeploymentDisplayMcpDeployment {
                        hash: hash.clone(),
                        agents: deployment
                            .agents
                            .iter()
                            .map(|(agent, options)| {
                                (
                                    agent.clone(),
                                    DeploymentDisplayMcpAgentOptions {
                                        security_scheme: options.security_scheme.clone(),
                                    },
                                )
                            })
                            .collect(),
                    },
                )
            })
            .or_else(|| {
                hash.map(|hash| {
                    (
                        domain.clone(),
                        DeploymentDisplayMcpDeployment {
                            hash: Some(hash),
                            agents: BTreeMap::new(),
                        },
                    )
                })
            })
    })
    .collect()
}

fn display_keys<'a, V>(
    mode: DeploymentDisplayMode,
    deployment: &'a BTreeMap<String, V>,
    diff: &'a BTreeMap<String, diff::BTreeMapDiffValue<<V as diff::Diffable>::DiffResult>>,
) -> Box<dyn Iterator<Item = &'a String> + 'a>
where
    V: diff::Diffable,
{
    match mode {
        DeploymentDisplayMode::ChangedOnly => Box::new(diff.keys()),
        DeploymentDisplayMode::Full => Box::new(deployment.keys()),
    }
}

fn display_agent_file_permissions(permissions: AgentFilePermissions) -> &'static str {
    match permissions {
        AgentFilePermissions::ReadOnly => "readonly",
        AgentFilePermissions::ReadWrite => "read-write",
    }
}

fn render_path(segments: &[PathSegment]) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }

    format!("/{}", segments.iter().map(render_path_segment).join("/"))
}

fn render_path_segment(segment: &PathSegment) -> String {
    match segment {
        PathSegment::Literal(segment) => segment.value.clone(),
        PathSegment::SystemVariable(segment) => format!("{{{}}}", segment.value),
        PathSegment::PathVariable(segment) => format!("{{{}}}", segment.variable_name),
        PathSegment::RemainingPathVariable(segment) => format!("{{*{}}}", segment.variable_name),
    }
}

fn render_http_method(method: &HttpMethod) -> &str {
    match method {
        HttpMethod::Get(_) => "GET",
        HttpMethod::Head(_) => "HEAD",
        HttpMethod::Post(_) => "POST",
        HttpMethod::Put(_) => "PUT",
        HttpMethod::Delete(_) => "DELETE",
        HttpMethod::Connect(_) => "CONNECT",
        HttpMethod::Options(_) => "OPTIONS",
        HttpMethod::Trace(_) => "TRACE",
        HttpMethod::Patch(_) => "PATCH",
        HttpMethod::Custom(method) => &method.value,
    }
}

fn render_agent_config_source(source: AgentConfigSource) -> &'static str {
    match source {
        AgentConfigSource::Local => "local",
        AgentConfigSource::Secret => "secret",
    }
}

fn mask_sensitive_key_value_for_deploy_diff(
    masking: MaskingConfig,
    key: &str,
    value: &str,
) -> String {
    if !masking.show_secrets && is_sensitive_key(key) {
        mask_secret_with_fingerprint(value)
    } else {
        value.to_string()
    }
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TryUpdateAllWorkersView {
    pub agents: Vec<AgentUpdateMeta>,
    /// Per-agent update errors, keyed by the (environment-unique) agent id.
    pub errors: BTreeMap<String, String>,
}

impl TryUpdateAllWorkersView {
    pub fn extend(&mut self, other: TryUpdateAllWorkersView) {
        self.agents.extend(other.agents);
        self.errors.extend(other.errors);
    }
}

impl StructuredOutput for TryUpdateAllWorkersView {
    const KIND: &'static str = "agent.update";
}

impl TextOutput for TryUpdateAllWorkersView {
    fn log(&self) {
        // NOP
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateMeta {
    pub component_name: ComponentName,
    pub agent_id: RawAgentId,
    pub from_revision: ComponentRevision,
    pub revision: ComponentRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeployConfig {
    pub plan: bool,
    pub stage: bool,
    pub approve_staging_steps: bool,
    pub full_diff: bool,
    pub force_build: Option<ForceBuildArg>,
    pub post_deploy_args: PostDeployArgs,
    pub repl_bridge_sdk_target: Option<GuestLanguage>,
    pub skip_build: bool,
}

pub enum DeploySummary {
    PlanOk,
    PlanUpToDate,
    PlanSkippedOnly,
    StagingOk, // Only for internal testing purposes
    DeployOk(PostDeployResult),
    DeployUpToDate(PostDeployResult),
    DeploySkippedOnly(PostDeployResult),
    RollbackOk(PostDeployResult),
    RollbackUpToDate(PostDeployResult),
}

#[derive(Error, Debug)]
pub enum DeployError {
    #[error("Cancelled")]
    Cancelled,
    #[error("Build error: {0}")]
    BuildError(anyhow::Error),
    #[error("Prepare error: {0}")]
    PrepareError(anyhow::Error),
    #[error("Plan error: {0}")]
    PlanError(anyhow::Error),
    #[error("Environment check error: {0}")]
    EnvironmentCheckError(anyhow::Error),
    #[error("Staging error: {0}")]
    StagingError(anyhow::Error),
    #[error("Deploy error: {0}")]
    DeployError(anyhow::Error),
    #[error("Rollback error: {0}")]
    RollbackError(anyhow::Error),
}

pub type DeployResult = Result<DeploySummary, DeployError>;

pub enum PostDeploySummary {
    NoRequestedChanges,
    NoDeployment,
    AgentUpdateOk,
    AgentRedeployOk,
    AgentDeleteOk,
}

#[derive(Error, Debug)]
pub enum PostDeployError {
    #[error("Prepare error: {0}")]
    PrepareError(anyhow::Error),
    #[error("Agent update error: {0}")]
    AgentUpdateError(anyhow::Error),
    #[error("Agent redeploy error: {0}")]
    AgentRedeployError(anyhow::Error),
    #[error("Agent delete error: {0}")]
    AgentDeleteError(anyhow::Error),
}

pub type PostDeployResult = Result<PostDeploySummary, PostDeployError>;

pub enum UpdateStagedComponentError {
    Service(ServiceError),
    Other(anyhow::Error),
}

pub type UpdateStagedComponentResult<T> = Result<T, UpdateStagedComponentError>;

/// Render a schema-native [`SchemaType`](golem_common::schema::schema_type::SchemaType)
/// for the given language. Config secret value types are inline (no graph
/// refs), so they are wrapped in a self-contained single-root graph.
fn render_schema_type_for_language(
    source_language: &SourceLanguage,
    typ: &golem_common::schema::schema_type::SchemaType,
) -> String {
    let graph = SchemaGraph {
        defs: vec![],
        root: typ.clone(),
    };
    render_type_for_language(source_language, &graph, &graph.root, true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentNewView {
    pub application_name: ApplicationName,
    pub environment_name: EnvironmentName,
    pub deployment: CurrentDeployment,
}

impl Masked for DeploymentNewView {}

impl MessageWithFields for DeploymentNewView {
    fn message(&self) -> String {
        "Created new deployment".to_owned()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Application", &self.application_name.0, format_id)
            .fmt_field("Environment", &self.environment_name.0, format_id)
            .fmt_field(
                "Environment ID",
                &self.deployment.environment_id,
                format_main_id,
            )
            .fmt_field(
                "Deployment Revision",
                &self.deployment.revision,
                format_main_id,
            );

        fields.fmt_field_optional(
            "Deployment Version",
            &self.deployment.version.0,
            !self.deployment.version.0.is_empty(),
            format_id,
        );

        fields
            .fmt_field("Hash", &self.deployment.deployment_hash, format_id)
            .field("Deploy Revision", &self.deployment.current_revision);

        fields.build()
    }
}

impl StructuredOutput for DeploymentNewView {
    const KIND: &'static str = "deploy.deployment";
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentListView {
    pub deployments: Vec<Deployment>,
}

impl StructuredOutput for DeploymentListView {
    const KIND: &'static str = "deploy.deployments";
}

impl TextOutput for DeploymentListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Deployment Revision").fixed_right(),
            Column::new("Deployment Version").fixed_right(),
            Column::new("Hash"),
        ]);
        for dep in &self.deployments {
            table.add_row(vec![
                dep.revision.get().to_string(),
                dep.version.0.clone(),
                dep.deployment_hash.to_string(),
            ]);
        }
        log_table(table);
    }
}

const DIFF_COLLAPSE_THRESHOLD: usize = 12;
const DIFF_COLLAPSE_KEEP_HEAD: usize = 3;
const DIFF_COLLAPSE_KEEP_TAIL: usize = 3;
const DIFF_COLLAPSE_DOTS: usize = 3;

impl TextOutput for DeploymentDiff {
    fn log(&self) {
        logln("");
        if !self.components.is_empty() {
            logln("Component changes:".log_color_help_group().to_string());
            for (component_name, component_diff) in &self.components {
                match component_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} component {}",
                            "create".green(),
                            component_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} component {}",
                            "delete".red(),
                            component_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        DiffForHashOf::HashDiff { .. } => {
                            logln(format!(
                                "  - {} component {}",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                        }
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} component {}, changes:",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                            if diff.wasm_changed {
                                logln("    - binary");
                            }
                            if !diff.agent_type_provision_config_changes.is_empty() {
                                logln("    - provision configs");
                                for (agent_type, change) in
                                    &diff.agent_type_provision_config_changes
                                {
                                    match change {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} agent type {}",
                                                "create".green(),
                                                agent_type.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} agent type {}",
                                                "delete".red(),
                                                agent_type.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(inner) => {
                                            logln(format!(
                                                "      - {} agent type {}:",
                                                "update".yellow(),
                                                agent_type.log_color_highlight()
                                            ));
                                            if let DiffForHashOf::ValueDiff { diff } = inner {
                                                log_provision_config_diff(diff);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            logln("");
        }
        if !self.http_api_deployments.is_empty() {
            logln(
                "HTTP API deployment changes:"
                    .log_color_help_group()
                    .to_string(),
            );
            for (domain, http_api_deployment_diff) in &self.http_api_deployments {
                match http_api_deployment_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "create".green(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "delete".red(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        DiffForHashOf::HashDiff { .. } => logln(format!(
                            "  - {} HTTP API deployment {}",
                            "update".yellow(),
                            domain.log_color_highlight()
                        )),
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} HTTP API deployment {}, changes:",
                                "update".yellow(),
                                domain.log_color_highlight()
                            ));
                            if diff.webhooks_url_changed {
                                logln("    - webhooks_url");
                            }
                            if diff.openapi_endpoint_changed {
                                logln("    - openapi_endpoint");
                            }
                            if !diff.agents_changes.is_empty() {
                                logln("    - agents");
                                for (agent_id, agent_diff) in &diff.agents_changes {
                                    match agent_diff {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "create".green(),
                                                agent_id.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "delete".red(),
                                                agent_id.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(diff) => {
                                            logln(format!(
                                                "      - {} agent {}, changes:",
                                                "update".yellow(),
                                                agent_id.log_color_highlight()
                                            ));
                                            if diff.security_scheme_changed {
                                                logln("        - security_scheme");
                                            }
                                            if diff.test_session_header_changed {
                                                logln("        - test_session_header");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            logln("");
        }
        if !self.mcp_deployments.is_empty() {
            logln("MCP deployment changes:".log_color_help_group().to_string());
            for (domain, mcp_deployment_diff) in &self.mcp_deployments {
                match mcp_deployment_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} MCP deployment {}",
                            "create".green(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} MCP deployment {}",
                            "delete".red(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        DiffForHashOf::HashDiff { .. } => {
                            logln(format!(
                                "  - {} MCP deployment {}",
                                "update".yellow(),
                                domain.log_color_highlight()
                            ));
                        }
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} MCP deployment {}, changes:",
                                "update".yellow(),
                                domain.log_color_highlight()
                            ));
                            if !diff.agents_changes.is_empty() {
                                logln("    - agents");
                                for (agent_id, agent_diff) in &diff.agents_changes {
                                    match agent_diff {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "create".green(),
                                                agent_id.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "delete".red(),
                                                agent_id.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(diff) => {
                                            logln(format!(
                                                "      - {} agent {}, changes:",
                                                "update".yellow(),
                                                agent_id.log_color_highlight()
                                            ));
                                            if diff.security_scheme_changed {
                                                logln("        - security_scheme");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            logln("");
        }
        if !self.remote_tools.is_empty() {
            logln("Remote tool changes:".log_color_help_group().to_string());
            for (tool_name, remote_tool_diff) in &self.remote_tools {
                let action = match remote_tool_diff {
                    BTreeMapDiffValue::Create => "create".green(),
                    BTreeMapDiffValue::Delete => "delete".red(),
                    BTreeMapDiffValue::Update(_) => "update".yellow(),
                };
                logln(format!(
                    "  - {} remote tool {}",
                    action,
                    tool_name.log_color_highlight(),
                ));
            }
            logln("");
        }
        if !self.published_tools.is_empty() {
            logln(
                "Deployment tool release changes:"
                    .log_color_help_group()
                    .to_string(),
            );
            for (tool_name, publication_diff) in &self.published_tools {
                let change = match publication_diff {
                    diff::BTreeSetDiffValue::Create => format!(
                        "include tool release reference {} in this deployment",
                        tool_name.log_color_highlight(),
                    )
                    .green()
                    .to_string(),
                    diff::BTreeSetDiffValue::Delete => format!(
                        "remove tool release {} from this deployment (release remains available)",
                        tool_name.log_color_highlight(),
                    )
                    .red()
                    .to_string(),
                };
                logln(format!("  - {change}"));
            }
            logln("");
        }
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        let _ = config;
        self.log();
        Ok(())
    }
}

impl StructuredOutput for DeploymentDiff {
    const KIND: &'static str = "deploy.diff";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl Masked for DeploymentDiff {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        if config.show_secrets {
            return Ok(self);
        }

        mask_deployment_diff_secrets(&mut self)?;
        Ok(self)
    }
}

fn mask_deployment_diff_secrets(diff: &mut DeploymentDiff) -> anyhow::Result<()> {
    for component_change in diff.components.values_mut() {
        let BTreeMapDiffValue::Update(component_diff) = component_change else {
            continue;
        };
        let DiffForHashOf::ValueDiff {
            diff: component_diff,
        } = component_diff
        else {
            continue;
        };

        for provision_config_change in component_diff
            .agent_type_provision_config_changes
            .values_mut()
        {
            let BTreeMapDiffValue::Update(provision_config_diff) = provision_config_change else {
                continue;
            };
            let DiffForHashOf::ValueDiff {
                diff: provision_config_diff,
            } = provision_config_diff
            else {
                continue;
            };
            mask_agent_type_provision_config_diff(provision_config_diff)?;
        }
    }

    for remote_tool_change in diff.remote_tools.values_mut() {
        let BTreeMapDiffValue::Update(remote_tool_diff) = remote_tool_change else {
            continue;
        };
        let DiffForHashOf::ValueDiff { diff: remote_tool } = remote_tool_diff else {
            continue;
        };

        remote_tool.provision.config = NormalizedJsonValue(mask_json_secret_for_deploy_diff(
            MaskingConfig::hide_secrets(),
            &remote_tool.provision.config,
        )?);
        remote_tool.provision.env =
            display_env(MaskingConfig::hide_secrets(), &remote_tool.provision.env);
        for plugin in &mut remote_tool.provision.plugins {
            plugin.parameters = plugin
                .parameters
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        mask_sensitive_key_value_for_deploy_diff(
                            MaskingConfig::hide_secrets(),
                            key,
                            value,
                        ),
                    )
                })
                .collect();
        }
        for binding in remote_tool.bindings.values_mut() {
            binding.parameters = NormalizedJsonValue(mask_json_secret_for_deploy_diff(
                MaskingConfig::hide_secrets(),
                &binding.parameters,
            )?);
        }
    }

    Ok(())
}

fn mask_agent_type_provision_config_diff(
    diff: &mut AgentTypeProvisionConfigDiff,
) -> anyhow::Result<()> {
    for env_change in diff.env_changes.values_mut() {
        mask_string_diff_update(env_change)?;
    }

    for config_change in diff.config_changes.values_mut() {
        mask_normalized_json_diff_update(config_change)?;
    }

    Ok(())
}

fn mask_string_diff_update(change: &mut BTreeMapDiffValue<String>) -> anyhow::Result<()> {
    if let BTreeMapDiffValue::Update(update) = change {
        *update = mask_secret_with_fingerprint(&serde_json::to_string(update)?);
    }
    Ok(())
}

fn mask_normalized_json_diff_update(
    change: &mut BTreeMapDiffValue<NormalizedJsonValue>,
) -> anyhow::Result<()> {
    if let BTreeMapDiffValue::Update(update) = change {
        *update = NormalizedJsonValue(serde_json::Value::String(mask_secret_with_fingerprint(
            &serde_json::to_string(update)?,
        )));
    }
    Ok(())
}

fn log_provision_config_diff(diff: &AgentTypeProvisionConfigDiff) {
    if !diff.env_changes.is_empty() {
        logln("        - env");
    }
    if !diff.config_changes.is_empty() {
        logln("        - agent config");
    }
    if !diff.file_changes.is_empty() {
        logln("        - files");
        for (path, file_diff) in &diff.file_changes {
            match file_diff {
                BTreeMapDiffValue::Create => logln(format!(
                    "          - {} {}",
                    "add".green(),
                    path.log_color_highlight()
                )),
                BTreeMapDiffValue::Delete => logln(format!(
                    "          - {} {}",
                    "remove".red(),
                    path.log_color_highlight()
                )),
                BTreeMapDiffValue::Update(inner) => {
                    if let DiffForHashOf::ValueDiff { diff } = inner {
                        let mut changes = vec![];
                        if diff.content_changed {
                            changes.push("content");
                        }
                        if diff.permissions_changed {
                            changes.push("permissions");
                        }
                        logln(format!(
                            "          - {} {} ({})",
                            "update".yellow(),
                            path.log_color_highlight(),
                            changes.join(", ")
                        ));
                    }
                }
            }
        }
    }
    if !diff.plugin_changes.is_empty() {
        // TODO: show plugin name/version once grant ID → name mapping is available
        logln(format!(
            "        - plugins ({} change(s))",
            diff.plugin_changes.len()
        ));
    }
    if diff.initial_permission_changed {
        logln("        - initial permissions");
    }
}

pub fn log_unified_diff(diff: &str) {
    for line in diff.lines() {
        log_unified_diff_line(classify_diff_line(line));
    }
}

pub fn log_unified_diff_for_path(path: &Path, diff: &str) {
    if is_compact_diff_path(path) {
        log_unified_diff_compact(diff);
    } else {
        log_unified_diff(diff);
    }
}

pub struct EnvironmentSetupPlanView<'a>(pub &'a EnvironmentSetupPlan);

impl Serialize for EnvironmentSetupPlanView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // EnvironmentSetupPlan.display is built with the active MaskingConfig.
        // This view serializes that prepared display and must not be constructed
        // from display data that skipped environment setup masking.
        self.0.display.serialize(serializer)
    }
}

pub struct DeployPlanView<'a> {
    pub deployment_diff: &'a DeploymentDiff,
    pub tool_publications: &'a ToolPublicationPlan,
    pub environment_setup: Option<&'a EnvironmentSetupPlan>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeployPlanFields<'a> {
    deployment_diff: &'a DeploymentDiff,
    tool_publications: &'a ToolPublicationPlan,
    environment_setup: Option<&'a EnvironmentSetupDisplay>,
}

impl Serialize for DeployPlanView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deployment_diff = self
            .deployment_diff
            .clone()
            .masked(MaskingConfig::hide_secrets())
            .map_err(serde::ser::Error::custom)?;

        DeployPlanFields {
            deployment_diff: &deployment_diff,
            tool_publications: self.tool_publications,
            environment_setup: self.environment_setup.map(|setup| &setup.display),
        }
        .serialize(serializer)
    }
}

impl TextOutput for DeployPlanView<'_> {
    fn log(&self) {
        let has_deployment_changes = !self.deployment_diff.components.is_empty()
            || !self.deployment_diff.http_api_deployments.is_empty()
            || !self.deployment_diff.mcp_deployments.is_empty()
            || !self.deployment_diff.remote_tools.is_empty()
            || !self.deployment_diff.published_tools.is_empty();

        if has_deployment_changes {
            self.deployment_diff.log();
        }

        if !self.tool_publications.entries.is_empty() {
            logln("Tool publication plan:".log_color_help_group().to_string());
            for entry in &self.tool_publications.entries {
                let coordinate = format!("{}@{}", entry.name, entry.version);
                let action = match entry.action {
                    ToolPublicationPlanAction::NoChange => entry.action.to_string().normal(),
                    ToolPublicationPlanAction::Publish => entry.action.to_string().green(),
                    ToolPublicationPlanAction::Conflict => entry.action.to_string().red(),
                };
                logln(format!(
                    "  - {} {}{}",
                    action,
                    coordinate.log_color_highlight(),
                    entry
                        .reason
                        .as_ref()
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default()
                ));
            }
            logln("");
        }

        if let Some(environment_setup) = self.environment_setup.map(EnvironmentSetupPlanView)
            && !environment_setup.0.display.is_empty()
        {
            environment_setup.log();
        }
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        let _ = config;
        self.log();
        Ok(())
    }
}

impl StructuredOutput for DeployPlanView<'_> {
    const KIND: &'static str = "deploy.plan";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deployment_diff = self
            .deployment_diff
            .clone()
            .masked(config)
            .map_err(serde::ser::Error::custom)?;

        DeployPlanFields {
            deployment_diff: &deployment_diff,
            tool_publications: self.tool_publications,
            environment_setup: self.environment_setup.map(|setup| &setup.display),
        }
        .serialize(serializer)
    }
}

impl TextOutput for EnvironmentSetupPlanView<'_> {
    fn log(&self) {
        let setup = self.0;

        if !setup.display.to_be_applied.is_empty() {
            logln(
                "Environment setup to apply:"
                    .log_color_help_group()
                    .to_string(),
            );
            if !setup.display.to_be_applied.secret_values.is_empty() {
                for key in setup.display.to_be_applied.secret_values.keys() {
                    logln(format!(
                        "  - create secret value {}",
                        key.log_color_highlight()
                    ));
                }
            }
            if !setup.display.to_be_applied.retry_policies.is_empty() {
                for key in setup.display.to_be_applied.retry_policies.keys() {
                    logln(format!(
                        "  - create retry policy {}",
                        key.log_color_highlight()
                    ));
                }
            }
            if !setup.display.to_be_applied.resources.is_empty() {
                for key in setup.display.to_be_applied.resources.keys() {
                    logln(format!("  - create resource {}", key.log_color_highlight()));
                }
            }
        }

        if !setup.display.skipped_already_exists.is_empty() {
            if !setup.display.to_be_applied.is_empty() {
                logln("");
            }
            logln(
                "Environment setup skipped because it already exists:"
                    .log_color_help_group()
                    .to_string(),
            );
            if !setup
                .display
                .skipped_already_exists
                .secret_values
                .is_empty()
            {
                for key in &setup.display.skipped_already_exists.secret_values {
                    logln(format!("  - secret value {}", key.log_color_highlight()));
                }
            }
            if !setup
                .display
                .skipped_already_exists
                .retry_policies
                .is_empty()
            {
                for key in &setup.display.skipped_already_exists.retry_policies {
                    logln(format!("  - retry policy {}", key.log_color_highlight()));
                }
            }
            if !setup.display.skipped_already_exists.resources.is_empty() {
                for key in &setup.display.skipped_already_exists.resources {
                    logln(format!("  - resource {}", key.log_color_highlight()));
                }
            }
        }
    }
}

impl StructuredOutput for EnvironmentSetupPlanView<'_> {
    const KIND: &'static str = "deploy.environment-setup-plan";
}

impl EnvironmentSetupPlanView<'_> {
    pub fn has_entries_to_apply(&self) -> bool {
        !self.0.display.to_be_applied.is_empty()
    }
}

fn is_compact_diff_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn log_unified_diff_compact(diff: &str) {
    let lines: Vec<DiffLine<'_>> = diff.lines().map(classify_diff_line).collect();
    let runs = regroup_diff_lines(&lines);

    for run in runs {
        render_diff_run(run);
    }
}

fn regroup_diff_lines<'a>(lines: &'a [DiffLine<'a>]) -> Vec<DiffRun<'a>> {
    let mut runs = Vec::new();

    for line in lines {
        match line {
            DiffLine::Added(_) => push_change_line(&mut runs, ChangeKind::Added, *line),
            DiffLine::Removed(_) => push_change_line(&mut runs, ChangeKind::Removed, *line),
            _ => push_other_line(&mut runs, *line),
        }
    }

    runs
}

fn push_change_line<'a>(runs: &mut Vec<DiffRun<'a>>, kind: ChangeKind, line: DiffLine<'a>) {
    match runs.last_mut() {
        Some(DiffRun::Change {
            kind: existing_kind,
            lines,
        }) if *existing_kind == kind => lines.push(line),
        _ => runs.push(DiffRun::Change {
            kind,
            lines: vec![line],
        }),
    }
}

fn push_other_line<'a>(runs: &mut Vec<DiffRun<'a>>, line: DiffLine<'a>) {
    match runs.last_mut() {
        Some(DiffRun::Other(lines)) => lines.push(line),
        _ => runs.push(DiffRun::Other(vec![line])),
    }
}

fn render_diff_run(run: DiffRun<'_>) {
    match run {
        DiffRun::Change { lines, .. } if lines.len() > DIFF_COLLAPSE_THRESHOLD => {
            let head_keep = DIFF_COLLAPSE_KEEP_HEAD.min(lines.len());
            let tail_keep = DIFF_COLLAPSE_KEEP_TAIL.min(lines.len() - head_keep);

            for line in lines.iter().take(head_keep) {
                log_unified_diff_line(*line);
            }

            for _ in 0..DIFF_COLLAPSE_DOTS {
                logln(".".dimmed().to_string());
            }

            for line in lines.iter().skip(lines.len() - tail_keep).take(tail_keep) {
                log_unified_diff_line(*line);
            }
        }
        DiffRun::Change { lines, .. } | DiffRun::Other(lines) => {
            for line in lines {
                log_unified_diff_line(line);
            }
        }
    }
}

fn log_unified_diff_line(line: DiffLine<'_>) {
    match line {
        DiffLine::Added(raw) => logln(raw.green().bold().to_string()),
        DiffLine::Removed(raw) => logln(raw.red().bold().to_string()),
        DiffLine::Hunk(raw) => logln(raw.bold().to_string()),
        DiffLine::Other(raw) => logln(raw),
    }
}

fn classify_diff_line(line: &str) -> DiffLine<'_> {
    if line.starts_with('+') && !line.starts_with("+++") {
        DiffLine::Added(line)
    } else if line.starts_with('-') && !line.starts_with("---") {
        DiffLine::Removed(line)
    } else if line.starts_with("@@") {
        DiffLine::Hunk(line)
    } else {
        DiffLine::Other(line)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ChangeKind {
    Added,
    Removed,
}

#[derive(Clone, Copy)]
enum DiffLine<'a> {
    Added(&'a str),
    Removed(&'a str),
    Hunk(&'a str),
    Other(&'a str),
}

enum DiffRun<'a> {
    Change {
        kind: ChangeKind,
        lines: Vec<DiffLine<'a>>,
    },
    Other(Vec<DiffLine<'a>>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployResultView {
    pub deployed: bool,
}

impl NoTextOutput for DeployResultView {}
impl TextOutput for DeployResultView {}

impl StructuredOutput for DeployResultView {
    const KIND: &'static str = "deploy";
}
