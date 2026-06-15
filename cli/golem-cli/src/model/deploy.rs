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
use crate::model::GuestLanguage;
use crate::model::component::{render_agent_constructor, render_data_schema};
use crate::model::text::component::is_sensitive_env_var_name;
use crate::model::worker::RawAgentId;
use golem_client::model::{AgentSecretDto, RetryPolicyDto};
use golem_common::model::agent::{
    AgentConfigSource, AgentMethod, AgentType, HttpEndpointDetails, HttpMethod, HttpMountDetails,
    PathSegment,
};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::component::{AgentFilePermissions, ComponentName, ComponentRevision};
use golem_common::model::deployment::{DeploymentAgentSecretDefault, DeploymentRetryPolicyDefault};
use golem_common::model::diff::{self, Hashable};
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionCreation};
use golem_wasm::analysis::AnalysedType as LegacyAnalysedType;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplay {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, DeploymentDisplayComponent>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub http_api_deployments: BTreeMap<String, DeploymentDisplayHttpApiDeployment>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_deployments: BTreeMap<String, DeploymentDisplayMcpDeployment>,
}

pub struct DeploymentDisplayContext<'a> {
    pub show_sensitive: bool,
    pub mode: DeploymentDisplayMode,
    pub deployment: &'a diff::Deployment,
    pub diff: &'a diff::DeploymentDiff,
    pub agent_types_by_component: &'a HashMap<String, Vec<AgentType>>,
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
    use golem_wasm::analysis::analysed_type::str as analysed_str;
    use uuid::Uuid;

    fn secret_dto(
        path: &[&str],
        secret_type: golem_wasm::analysis::AnalysedType,
        value: Option<serde_json::Value>,
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
    fn environment_setup_secret_type_rendering_matches_between_manifest_and_environment() {
        let mut secret_types = BTreeMap::new();
        secret_types.insert("superSecret".to_string(), analysed_str());

        let plan = build_environment_setup_plan(
            vec![DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec!["superSecret".to_string()]),
                secret_value: serde_json::json!("same-value"),
            }],
            Vec::new(),
            Vec::new(),
            vec![secret_dto(
                &["superSecret"],
                analysed_str(),
                Some(serde_json::json!("same-value")),
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
        secret_types.insert("createSecret".to_string(), analysed_str());
        secret_types.insert("existingSecret".to_string(), analysed_str());

        let plan = build_environment_setup_plan(
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
                analysed_str(),
                Some(serde_json::json!("env")),
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
    agent_types_by_component: &HashMap<String, Vec<AgentType>>,
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
    resolved_agent_secret_defaults: Vec<DeploymentAgentSecretDefault>,
    retry_policy_defaults: Vec<DeploymentRetryPolicyDefault>,
    resource_defaults: Vec<ResourceDefinitionCreation>,
    current_agent_secrets: Vec<AgentSecretDto>,
    current_retry_policies: Vec<RetryPolicyDto>,
    current_resources: Vec<ResourceDefinition>,
    secret_types_by_path: &BTreeMap<String, golem_wasm::analysis::AnalysedType>,
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
                        .map(|typ| render_legacy_type_for_language(source_language, typ))
                        .unwrap_or_else(|| "unknown".to_string()),
                    value: masked_json_value(&default.secret_value)?,
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let current_secret_values = current_agent_secrets
        .into_iter()
        .map(|secret| {
            let value = match secret.secret_value {
                Some(value) => masked_json_value(&value)?,
                None => serde_json::Value::Null,
            };
            Ok((
                secret.path.to_string(),
                EnvironmentSetupSecretValueDisplay {
                    secret_type: render_legacy_type_for_language(
                        source_language,
                        &secret.secret_type,
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

impl DeploymentDisplay {
    pub fn from_context(ctx: DeploymentDisplayContext<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            components: display_components(&ctx)?,
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
            && self.http_api_deployments.is_empty()
            && self.mcp_deployments.is_empty()
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
                        display_agent_type(ctx.show_sensitive, agent, component)?,
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
    show_sensitive: bool,
    agent: &AgentType,
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
        config_defaults: display_config_defaults(show_sensitive, agent, provision_config)?,
        env: provision_config
            .map(|config| display_env(show_sensitive, &config.env))
            .unwrap_or_default(),
        files: provision_config
            .map(display_files)
            .transpose()?
            .unwrap_or_default(),
        plugins: provision_config
            .map(|config| display_plugins(show_sensitive, config))
            .unwrap_or_default(),
        http_mount: agent.http_mount.as_ref().map(display_http_mount),
        methods: agent
            .methods
            .iter()
            .map(|method| (method.name.clone(), display_method(&lang, method)))
            .collect(),
        dependencies: agent
            .dependencies
            .iter()
            .map(|dependency| dependency.type_name.clone())
            .collect(),
    })
}

fn display_config_declarations(
    agent: &AgentType,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayConfigDeclaration>> {
    let lang = SourceLanguage::from(agent.source_language.as_str());

    agent
        .config
        .iter()
        .map(|config| {
            // Adapt legacy AnalysedType at the boundary.
            let value_type = match golem_common::schema::adapters::analysed_type_to_schema_graph(
                &config.value_type,
            ) {
                Ok(graph) => {
                    let root = graph.root.clone();
                    render_type_for_language(&lang, &graph, &root, true)
                }
                Err(_) => "<unknown>".to_string(),
            };
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
    show_sensitive: bool,
    agent: &AgentType,
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

        let rendered_value = if is_secret && !show_sensitive {
            masked_json_value(value)?
        } else {
            serde_json::to_value(value)?
        };

        result.insert((*path).clone(), rendered_value);
    }

    Ok(result)
}

fn display_env(show_sensitive: bool, env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                mask_sensitive_value(show_sensitive, key, value),
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
    show_sensitive: bool,
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
                        mask_sensitive_value(show_sensitive, key, value),
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

fn display_method(lang: &SourceLanguage, method: &AgentMethod) -> DeploymentDisplayMethod {
    let output = render_data_schema(&method.output_schema, lang, false);
    let signature = if output.is_empty() {
        format!(
            "{}({})",
            method.name,
            render_data_schema(&method.input_schema, lang, true)
        )
    } else {
        format!(
            "{}({}) -> {}",
            method.name,
            render_data_schema(&method.input_schema, lang, true),
            output
        )
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

fn mask_sensitive_value(show_sensitive: bool, key: &str, value: &str) -> String {
    if !show_sensitive && is_sensitive_env_var_name(show_sensitive, key) {
        masked_secret(value)
    } else {
        value.to_string()
    }
}

fn masked_json_value(value: &impl Serialize) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::String(masked_secret(
        &serde_json::to_string(value)?,
    )))
}

fn masked_secret(value: &str) -> String {
    format!(
        "<masked-secret:{}>",
        blake3::hash(value.as_bytes()).to_hex()
    )
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TryUpdateAllWorkersResult {
    pub triggered: Vec<WorkerUpdateAttempt>,
    pub failed: Vec<WorkerUpdateAttempt>,
}

impl TryUpdateAllWorkersResult {
    pub fn extend(&mut self, other: TryUpdateAllWorkersResult) {
        self.triggered.extend(other.triggered);
        self.failed.extend(other.failed);
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerUpdateAttempt {
    pub component_name: ComponentName,
    pub target_revision: ComponentRevision,
    pub agent_name: RawAgentId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

/// Boundary helper: render a legacy [`LegacyAnalysedType`] via the schema-typed
/// type renderer by first adapting it into a [`golem_common::schema::SchemaGraph`].
fn render_legacy_type_for_language(
    source_language: &SourceLanguage,
    typ: &LegacyAnalysedType,
) -> String {
    match golem_common::schema::adapters::analysed_type_to_schema_graph(typ) {
        Ok(graph) => {
            let root = graph.root.clone();
            render_type_for_language(source_language, &graph, &root, true)
        }
        Err(_) => "<unknown>".to_string(),
    }
}
