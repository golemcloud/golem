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

use crate::agent_id_display::SourceLanguage;
use crate::agent_id_display::render_type_for_language;
use crate::log::LogColorize;
use crate::model::agent::RawAgentId;
use crate::model::app::{ComponentLayerId, ComponentLayerProperties};
use crate::model::app_raw;
use crate::model::cli_output::StructuredOutput;
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::masking::{
    Masked, MaskingConfig, is_sensitive_key, mask_secret, mask_sensitive_map,
    mask_typed_agent_config_entries,
};
use crate::model::text_format::*;
use chrono::{DateTime, Utc};
use colored::Colorize;
use colored::control::SHOULD_COLORIZE;
use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
use golem_common::model::agent::{AgentConfigSource, AgentFileContentHash, AgentTypeName};
use golem_common::model::card::recipient::{RecipientMonomorphizationContext, RecipientPattern};
use golem_common::model::card::{
    PolymorphicCard, PolymorphicManifestPermissionPattern,
    parse_polymorphic_manifest_permission_grant, parse_polymorphic_permission,
};
use golem_common::model::component::{
    AgentConfigEntryDto, ComponentDto, ComponentId, ComponentRevision,
};
use golem_common::model::component::{
    AgentFileOptions, AgentFilePath, AgentTypeInitialPermissions, AgentTypeProvisionConfigCreation,
    ArchiveFilePath, PluginInstallation,
};
use golem_common::model::component::{AgentFilePermissions, ComponentName};
use golem_common::model::component::{InitialAgentFile, InstalledPlugin};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::{ToolDeploymentMetadata, ToolName};
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_common::model::{diff, tool};
use golem_common::schema::agent::{AgentTypeSchema, FieldSource, InputSchema, OutputSchema};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::tool::Tool;
use heck::{ToLowerCamelCase, ToSnakeCase};
use itertools::Itertools;
use serde::Serializer;
use serde::ser::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedInitialPermissionCard {
    pub lower_positive: Vec<PolymorphicManifestPermissionPattern>,
    pub lower_negative: Vec<PolymorphicManifestPermissionPattern>,
    pub upper_positive: Vec<PolymorphicManifestPermissionPattern>,
    pub upper_negative: Vec<PolymorphicManifestPermissionPattern>,
}

impl ParsedInitialPermissionCard {
    pub fn from_grant_strings(
        lower_positive: Vec<String>,
        lower_negative: Vec<String>,
        upper_positive: Vec<String>,
        upper_negative: Vec<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            lower_positive: parse_manifest_grants(lower_positive)?,
            lower_negative: parse_manifest_grants(lower_negative)?,
            upper_positive: parse_manifest_grants(upper_positive)?,
            upper_negative: parse_manifest_grants(upper_negative)?,
        })
    }

    pub fn resolve_recipients(
        self,
        context: &RecipientMonomorphizationContext,
    ) -> AgentTypeInitialPermissions {
        AgentTypeInitialPermissions::from_patterns(
            self.lower_positive
                .into_iter()
                .map(|grant| grant.monomorphize_recipient(context))
                .collect(),
            self.lower_negative
                .into_iter()
                .map(|grant| grant.monomorphize_recipient(context))
                .collect(),
            self.upper_positive
                .into_iter()
                .map(|grant| grant.monomorphize_recipient(context))
                .collect(),
            self.upper_negative
                .into_iter()
                .map(|grant| grant.monomorphize_recipient(context))
                .collect(),
        )
    }
}

fn parse_manifest_grants(
    grants: Vec<String>,
) -> anyhow::Result<Vec<PolymorphicManifestPermissionPattern>> {
    grants
        .into_iter()
        .map(|grant| {
            parse_polymorphic_manifest_permission_grant(&grant)
                .map_err(|err| anyhow::anyhow!("invalid grant '{}': {}", grant, err))
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .map(|grants| grants.into_iter().flatten().collect())
}

pub enum ComponentRevisionSelection<'a> {
    ByAgentId(&'a RawAgentId),
    ByExplicitRevision(ComponentRevision),
}

impl<'a> From<&'a RawAgentId> for ComponentRevisionSelection<'a> {
    fn from(value: &'a RawAgentId) -> Self {
        Self::ByAgentId(value)
    }
}

impl From<ComponentRevision> for ComponentRevisionSelection<'_> {
    fn from(value: ComponentRevision) -> Self {
        Self::ByExplicitRevision(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentNameMatchKind {
    AppCurrentDir,
    App,
    Unknown,
}

pub struct SelectedComponents {
    pub environment: ResolvedEnvironmentIdentity,
    pub component_names: Vec<ComponentName>,
}

pub enum ComponentUpsertResult {
    Skipped,
    Added(ComponentDto),
    Updated(ComponentDto),
}

impl ComponentUpsertResult {
    pub fn into_component(self) -> Option<ComponentDto> {
        match self {
            ComponentUpsertResult::Skipped => None,
            ComponentUpsertResult::Added(component) => Some(component),
            ComponentUpsertResult::Updated(component) => Some(component),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentView {
    pub component_name: ComponentName,
    pub component_id: ComponentId,
    pub component_version: Option<String>,
    pub component_revision: u64,
    pub component_size: u64,
    pub created_at: DateTime<Utc>,
    pub environment_id: EnvironmentId,
    pub exports: Vec<String>,
    pub agent_types: Vec<AgentTypeSchema>,
    pub agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    pub tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
}

impl Masked for ComponentView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        if config.show_secrets {
            return Ok(self);
        }

        let secret_config_paths_by_agent_type = self
            .agent_types
            .iter()
            .map(|agent_type| {
                (
                    agent_type.type_name.0.clone(),
                    agent_type
                        .config
                        .iter()
                        .filter(|config| config.source == AgentConfigSource::Secret)
                        .map(|config| config.path.join("."))
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        for (agent_type_name, provision_config) in &mut self.agent_type_provision_configs {
            provision_config.env = mask_sensitive_map(config, &provision_config.env);

            for plugin in &mut provision_config.plugins {
                plugin.parameters = mask_sensitive_map(config, &plugin.parameters);
            }

            if let Some(secret_paths) = secret_config_paths_by_agent_type.get(&agent_type_name.0) {
                provision_config.config =
                    mask_typed_agent_config_entries(config, &provision_config.config, secret_paths);
            }
        }

        for tool in self.tools.values_mut() {
            mask_json_leaf_values(&mut tool.provision.config.0);
            tool.provision.env = mask_sensitive_map(config, &tool.provision.env);
            for plugin in &mut tool.provision.plugins {
                plugin.parameters = mask_sensitive_map(config, &plugin.parameters);
            }
        }

        Ok(self)
    }
}

impl ComponentView {
    pub fn new(value: ComponentDto) -> Self {
        let agent_types = value.metadata.agent_types().to_vec();
        let exports = { show_exported_agents(&agent_types, true, true) };

        ComponentView {
            component_name: value.component_name,
            component_id: value.id,
            component_version: value.metadata.root_package_version().clone(),
            component_revision: value.revision.into(),
            component_size: value.component_size,
            created_at: value.created_at,
            environment_id: value.environment_id,
            exports,
            agent_types,
            agent_type_provision_configs: value.metadata.agent_type_provision_configs().clone(),
            tools: value.metadata.tools().clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentTypeManifestProvisionConfig {
    pub env: BTreeMap<String, String>,
    pub config: Vec<AgentConfigEntryDto>,
    pub initial_card: Option<ParsedInitialPermissionCard>,
    pub files_source: PathBuf,
    pub files: Vec<app_raw::InitialComponentFile>,
    pub plugins: Vec<app_raw::PluginInstallation>,
}

impl AgentTypeManifestProvisionConfig {
    pub fn to_provision_config_creation(
        &self,
        resolved_plugins: Vec<PluginInstallation>,
        initial_permissions: AgentTypeInitialPermissions,
    ) -> anyhow::Result<AgentTypeProvisionConfigCreation> {
        let files = self
            .files
            .iter()
            .map(|f| {
                let archive_path = ArchiveFilePath(f.target_path.clone());
                let options = AgentFileOptions {
                    target_path: AgentFilePath(f.target_path.clone()),
                    permissions: f.permissions.unwrap_or(AgentFilePermissions::ReadOnly),
                };
                (archive_path, options)
            })
            .collect();
        Ok(AgentTypeProvisionConfigCreation {
            initial_permissions,
            env: self.env.clone(),
            config: self.config.clone(),
            files,
            plugin_installations: resolved_plugins,
        })
    }

    pub fn to_initial_permission(
        &self,
        context: &RecipientMonomorphizationContext,
    ) -> AgentTypeInitialPermissions {
        let mut permissions = self
            .initial_card
            .clone()
            .map(|card| card.resolve_recipients(context))
            .unwrap_or_else(|| {
                AgentTypeInitialPermissions::default_for_recipient(initial_permission_recipient(
                    context,
                ))
            });
        let recipient = initial_permission_recipient(context).render();
        for file in &self.files {
            let path = file.target_path.as_abs_str();
            let descendants = if path == "/" {
                "/**".to_string()
            } else {
                format!("{path}/**")
            };
            let verbs: &[&str] = match file.permissions.unwrap_or_default() {
                AgentFilePermissions::ReadOnly => &["read", "stat", "list"],
                AgentFilePermissions::ReadWrite => &["read", "stat", "list", "write", "delete"],
            };
            for verb in verbs {
                for resource in [path, descendants.as_str()] {
                    permissions.lower_bound.positive.push(
                        parse_polymorphic_permission(&format!(
                            "filesystem(?agent) @ {recipient} : {verb} : {resource}"
                        ))
                        .expect("canonical initial file path must form a valid permission"),
                    );
                }
            }
        }
        permissions
    }
}

pub fn initial_permission_from_manifest_card(
    initial_card: &app_raw::ManifestInitialCard,
) -> anyhow::Result<ParsedInitialPermissionCard> {
    ParsedInitialPermissionCard::from_grant_strings(
        initial_card.lower_bound.positive.clone(),
        initial_card.lower_bound.negative.clone(),
        initial_card.upper_bound.positive.clone(),
        initial_card.upper_bound.negative.clone(),
    )
    .map_err(anyhow::Error::msg)
}

#[derive(Debug)]
pub struct ComponentDeployProperties {
    pub wasm_path: PathBuf,
    pub agent_types: Vec<AgentTypeSchema>,
    pub tools: Vec<Tool>,
    pub agent_type_configs: BTreeMap<AgentTypeName, AgentTypeManifestProvisionConfig>,
    pub tool_deployment_configs: BTreeMap<ToolName, ToolManifestDeploymentConfig>,
}

#[derive(Debug)]
pub struct DeployableManifestComponents {
    pub components: BTreeMap<ComponentName, ComponentDeployProperties>,
    pub remote_tool_deployments: Vec<tool::RemoteToolDeployment>,
    pub diffable_remote_tool_deployments:
        BTreeMap<String, diff::HashOf<diff::RemoteToolDeployment>>,
    pub published_tools: BTreeSet<ToolName>,
    pub pending_remote_initial_files: Vec<PendingRemoteInitialFile>,
}

#[derive(Debug)]
pub struct PendingRemoteInitialFile {
    pub content: Arc<tempfile::NamedTempFile>,
    pub content_hash: AgentFileContentHash,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct ToolManifestProvisionConfig {
    pub config: golem_common::model::json::NormalizedJsonValue,
    pub env: BTreeMap<String, String>,
    pub files: Vec<crate::model::app::ToolProvisionFile>,
    pub plugins: Vec<app_raw::PluginInstallation>,
}

#[derive(Clone, Debug)]
pub struct ToolManifestDeploymentConfig {
    pub provision: ToolManifestProvisionConfig,
    pub environment_binding: Option<golem_common::model::tool::ToolBindingInput>,
    pub agent_bindings: BTreeMap<AgentTypeName, golem_common::model::tool::ToolBindingInput>,
}

pub fn initial_permission_recipient_context(
    environment: &ResolvedEnvironmentIdentity,
    component_name: &ComponentName,
    agent_type_name: &AgentTypeName,
) -> RecipientMonomorphizationContext {
    RecipientMonomorphizationContext {
        account: environment.server_environment.owner_account_email.clone(),
        application: environment.application_name.clone(),
        environment: environment.environment_name.clone(),
        component: component_name.clone(),
        agent_type: agent_type_name.clone(),
    }
}

pub fn initial_permission_recipient(
    context: &RecipientMonomorphizationContext,
) -> RecipientPattern {
    RecipientPattern::Agent {
        account: context.account.clone(),
        application: context.application.clone(),
        environment: context.environment.clone(),
        component: context.component.clone(),
        agent_type: context.agent_type.clone(),
    }
}

pub fn show_exported_agents(
    agents: &[AgentTypeSchema],
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> Vec<String> {
    agents
        .iter()
        .flat_map(|agent| render_exported_agent(agent, wrapper_naming, show_dummy_return_type))
        .collect()
}

pub fn show_exported_agent_constructors(
    agents: &[AgentTypeSchema],
    wrapper_naming: bool,
) -> Vec<String> {
    agents
        .iter()
        .map(|c| render_agent_constructor(c, wrapper_naming, true))
        .collect()
}

fn render_exported_agent(
    agent: &AgentTypeSchema,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> Vec<String> {
    let lang = SourceLanguage::from(agent.source_language.as_str());
    let mut result = Vec::new();
    result.push(render_agent_constructor_with_lang(
        agent,
        wrapper_naming,
        show_dummy_return_type,
        &lang,
    ));
    let agent_id = if wrapper_naming {
        format!("{}.", agent.type_name.0)
    } else {
        "  ".to_string()
    };
    for method in &agent.methods {
        let output = render_output_schema(&agent.schema, &method.output_schema, &lang);
        let input = render_input_schema(&agent.schema, &method.input_schema, &lang, true);
        if output.is_empty() {
            result.push(format!("{}{}({})", agent_id, method.name, input));
        } else {
            result.push(format!(
                "{}{}({}) -> {}",
                agent_id, method.name, input, output
            ));
        }
    }

    result
}

pub fn render_agent_constructor(
    agent: &AgentTypeSchema,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> String {
    let lang = SourceLanguage::from(agent.source_language.as_str());
    render_agent_constructor_with_lang(agent, wrapper_naming, show_dummy_return_type, &lang)
}

fn render_agent_constructor_with_lang(
    agent: &AgentTypeSchema,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
    lang: &SourceLanguage,
) -> String {
    let dummy_return_type = if show_dummy_return_type {
        " agent constructor"
    } else {
        ""
    };
    let input = render_input_schema(&agent.schema, &agent.constructor.input_schema, lang, true);
    if wrapper_naming {
        format!(
            "{}({}){}",
            agent.type_name.0.clone(),
            input,
            dummy_return_type
        )
    } else {
        format!("{}({}){}", agent.type_name, input, dummy_return_type)
    }
}

fn render_param_name(name: &str, lang: &SourceLanguage) -> String {
    match lang {
        SourceLanguage::Rust => name.to_snake_case(),
        SourceLanguage::TypeScript
        | SourceLanguage::Scala
        | SourceLanguage::MoonBit
        | SourceLanguage::Other(_) => name.to_lower_camel_case(),
    }
}

pub(crate) fn render_input_schema(
    graph: &SchemaGraph,
    input: &InputSchema,
    lang: &SourceLanguage,
    show_param_names: bool,
) -> String {
    input
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| {
            let rendered_type = render_type_for_language(lang, graph, &field.schema, true);
            if show_param_names {
                format!(
                    "{}: {}",
                    render_param_name(&field.name, lang),
                    rendered_type
                )
            } else {
                rendered_type
            }
        })
        .join(", ")
}

pub(crate) fn render_output_schema(
    graph: &SchemaGraph,
    output: &OutputSchema,
    lang: &SourceLanguage,
) -> String {
    match output {
        OutputSchema::Unit => String::new(),
        OutputSchema::Single(ty) => render_type_for_language(lang, graph, ty, true),
    }
}

pub fn agent_interface_name(component: &ComponentDto, agent_type_name: &str) -> Option<String> {
    match (
        component.metadata.root_package_name(),
        component.metadata.root_package_version(),
    ) {
        (Some(name), Some(version)) => Some(format!("{}/{}@{}", name, agent_type_name, version)),
        (Some(name), None) => Some(format!("{}/{}", name, agent_type_name)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentTypeManifestProvisionConfig, ParsedInitialPermissionCard, app_raw};
    use golem_common::model::account::AccountEmail;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::card::owner::{AgentOwnerLeafPattern, PolymorphicAgentOwnerPattern};
    use golem_common::model::card::recipient::{
        PolymorphicAgentRecipientPattern, PolymorphicRecipientPattern,
        RecipientMonomorphizationContext, RecipientPattern,
    };
    use golem_common::model::card::{
        AgentMethodName, AgentResourcePattern, AgentVerb, PolymorphicManifestPermissionPattern,
    };
    use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath, ComponentName};
    use golem_common::model::environment::EnvironmentName;
    use test_r::test;

    fn manifest_card() -> ParsedInitialPermissionCard {
        ParsedInitialPermissionCard::from_grant_strings(
            vec![
                "agent(?env/payment-svc/*) @ ?component/* : * : *".to_string(),
                "agent(?env/payment-svc/PaymentAgent(*)) @ ?agent : invoke : charge".to_string(),
            ],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn parsed_initial_permission_card_golden() {
        let card = manifest_card();

        assert_eq!(card.lower_positive.len(), 2);
        match &card.lower_positive[0] {
            PolymorphicManifestPermissionPattern::Agent(pattern) => {
                assert_eq!(
                    pattern.owner,
                    PolymorphicAgentOwnerPattern::EnvComponentAgents {
                        component: ComponentName("payment-svc".to_string())
                    }
                );
                assert_eq!(
                    pattern.recipient,
                    PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::ComponentAgents
                    )
                );
                assert_eq!(pattern.verb, None);
                assert_eq!(pattern.resource, AgentResourcePattern::Any);
            }
            other => panic!("unexpected first grant: {other:?}"),
        }
        match &card.lower_positive[1] {
            PolymorphicManifestPermissionPattern::Agent(pattern) => {
                assert_eq!(
                    pattern.owner,
                    PolymorphicAgentOwnerPattern::EnvAgent {
                        component: ComponentName("payment-svc".to_string()),
                        agent: AgentOwnerLeafPattern::AgentTypeWildcard(AgentTypeName(
                            "PaymentAgent".to_string()
                        ))
                    }
                );
                assert_eq!(
                    pattern.recipient,
                    PolymorphicRecipientPattern::Agent(PolymorphicAgentRecipientPattern::Agent)
                );
                assert_eq!(pattern.verb, Some(AgentVerb::Invoke));
                assert_eq!(
                    pattern.resource,
                    AgentResourcePattern::Method(AgentMethodName("charge".to_string()))
                );
            }
            other => panic!("unexpected second grant: {other:?}"),
        }
    }

    #[test]
    fn initial_permission_card_expands_legacy_agent_debug_alias() {
        let card = ParsedInitialPermissionCard::from_grant_strings(
            vec!["agent(?env/payment-svc/PaymentAgent(*)) @ ?agent : debug : *".to_string()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();

        assert_eq!(card.lower_positive.len(), 8);
        assert_eq!(
            card.lower_positive
                .iter()
                .map(|grant| grant.render().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "oplog(?env/payment-svc/PaymentAgent(*)) @ ?agent : read : *",
                "agent(?env/payment-svc/PaymentAgent(*)) @ ?agent : view : ",
                "filesystem(?env/payment-svc/PaymentAgent(*)) @ ?agent : read : /**",
                "env(?env/payment-svc/PaymentAgent(*)) @ ?agent : read : *",
                "config(?env/payment-svc/PaymentAgent(*)) @ ?agent : read : *",
                "agent(?env/payment-svc/PaymentAgent(*)) @ ?agent : fork : ",
                "agent(?env/payment-svc/PaymentAgent(*)) @ ?agent : interrupt : ",
                "agent(?env/payment-svc/PaymentAgent(*)) @ ?agent : resume : ",
            ]
        );
    }

    #[test]
    fn parsed_initial_permission_card_monomorphizes_recipients() {
        let context = test_context();

        let initial_permission = manifest_card().resolve_recipients(&context);
        let rendered = initial_permission
            .lower_bound
            .positive
            .into_iter()
            .map(|p| p.render().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "agent(?env/payment-svc/*) @ account@example.com/shop/prod/cart-svc/* : * : *",
                "agent(?env/payment-svc/PaymentAgent(*)) @ account@example.com/shop/prod/cart-svc/Cart : invoke : charge",
            ]
        );
    }

    #[test]
    fn default_initial_permission_card_uses_agent_recipient() {
        let context = test_context();
        let initial_permission =
            AgentTypeManifestProvisionConfig::default().to_initial_permission(&context);
        let expected = RecipientPattern::Agent {
            account: context.account,
            application: context.application,
            environment: context.environment,
            component: context.component,
            agent_type: context.agent_type,
        };

        assert!(
            initial_permission
                .lower_bound
                .positive
                .iter()
                .all(|permission| permission.recipient() == &expected)
        );
    }

    #[test]
    fn initial_files_add_matching_filesystem_permissions() {
        let context = test_context();
        let config = AgentTypeManifestProvisionConfig {
            files: vec![app_raw::InitialComponentFile {
                source_path: "assets".to_string(),
                target_path: CanonicalFilePath::from_abs_str("/assets").unwrap(),
                permissions: Some(AgentFilePermissions::ReadWrite),
            }],
            ..Default::default()
        };

        let permissions = config.to_initial_permission(&context);
        assert!(permissions.upper_bound.positive.is_empty());

        let rendered = permissions
            .lower_bound
            .positive
            .into_iter()
            .filter_map(|permission| permission.render().ok())
            .filter(|permission| permission.contains(": /assets"))
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : read : /assets",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : read : /assets/**",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : stat : /assets",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : stat : /assets/**",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : list : /assets",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : list : /assets/**",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : write : /assets",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : write : /assets/**",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : delete : /assets",
                "filesystem(?agent) @ account@example.com/shop/prod/cart-svc/Cart : delete : /assets/**",
            ]
        );
    }

    fn test_context() -> RecipientMonomorphizationContext {
        RecipientMonomorphizationContext {
            account: AccountEmail::new("Account@Example.com"),
            application: ApplicationName("shop".to_string()),
            environment: EnvironmentName("prod".to_string()),
            component: ComponentName("cart-svc".to_string()),
            agent_type: AgentTypeName("Cart".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentListView {
    pub components: Vec<ComponentView>,
}

impl Masked for ComponentListView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        self.components = self
            .components
            .into_iter()
            .map(|component| component.masked(config))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(self)
    }
}

impl StructuredOutput for ComponentListView {
    const KIND: &'static str = "component.list";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl TextOutput for ComponentListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Name"),
            Column::new("Revision").fixed_right(),
            Column::new("Version").fixed_right(),
            Column::new("Size").fixed_right(),
            Column::new("Exports").fixed_right(),
        ]);
        for comp in &self.components {
            table.add_row(vec![
                comp.component_name.to_string(),
                comp.component_revision.to_string(),
                comp.component_version.clone().unwrap_or_default(),
                format_binary_size(&comp.component_size),
                comp.exports.len().to_string(),
            ]);
        }
        log_table(table);
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        self.masked(config)?.log();
        Ok(())
    }
}

fn component_view_fields(view: &ComponentView) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Component name", &view.component_name, format_main_id)
        .fmt_field("Component ID", &view.component_id, format_id)
        .fmt_field("Component revision", &view.component_revision, format_id)
        .fmt_field_option("Component version", &view.component_version, format_id)
        .fmt_field("Environment ID", &view.environment_id, format_id)
        .fmt_field("Component size", &view.component_size, format_binary_size)
        .fmt_field("Created at", &view.created_at, |d| d.to_string())
        .fmt_field("Exports", &view.exports, |e| format_exports(e.as_slice()));

    for (agent_type_name, provision_config) in &view.agent_type_provision_configs {
        let prefix = format!("[{}] ", agent_type_name.0);
        fields
            .fmt_field_optional(
                &format!("{}Environment", prefix),
                &provision_config.env,
                !provision_config.env.is_empty(),
                format_env,
            )
            .fmt_field_optional(
                &format!("{}Agent config", prefix),
                provision_config.config.as_slice(),
                !provision_config.config.is_empty(),
                format_typed_config,
            )
            .fmt_field_optional(
                &format!("{}Initial file system", prefix),
                provision_config.files.as_slice(),
                !provision_config.files.is_empty(),
                format_files,
            )
            .fmt_field_optional(
                &format!("{}Plugins", prefix),
                provision_config.plugins.as_slice(),
                !provision_config.plugins.is_empty(),
                format_plugins,
            )
            .fmt_field_optional(
                &format!("{}Initial permissions", prefix),
                &provision_config.initial_permissions,
                !initial_permission_is_empty(&provision_config.initial_permissions),
                format_initial_permission,
            );
    }

    fields.build()
}

fn initial_permission_is_empty(card: &PolymorphicCard) -> bool {
    card.lower_positive.is_empty()
        && card.lower_negative.is_empty()
        && card.upper_positive.is_empty()
        && card.upper_negative.is_empty()
}

fn format_initial_permission(card: &PolymorphicCard) -> String {
    let mut sections = Vec::new();
    push_initial_permission_section(&mut sections, "lower positive", &card.lower_positive);
    push_initial_permission_section(&mut sections, "lower negative", &card.lower_negative);
    push_initial_permission_section(&mut sections, "upper positive", &card.upper_positive);
    push_initial_permission_section(&mut sections, "upper negative", &card.upper_negative);
    sections.join("\n")
}

fn push_initial_permission_section(
    sections: &mut Vec<String>,
    name: &str,
    permissions: &[golem_common::model::card::PolymorphicPermissionPattern],
) {
    if permissions.is_empty() {
        return;
    }

    let grants = permissions
        .iter()
        .map(|permission| {
            permission
                .render()
                .unwrap_or_else(|error| format!("<failed to render grant: {error}>"))
        })
        .map(|grant| format!("  - {grant}"))
        .collect::<Vec<_>>()
        .join("\n");
    sections.push(format!("{name}:\n{grants}"));
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentCreateView(pub ComponentView);

impl Masked for ComponentCreateView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for ComponentCreateView {
    fn message(&self) -> String {
        format!(
            "Created new component {}",
            format_message_highlight(&self.0.component_name)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        component_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentUpdateView(pub ComponentView);

impl Masked for ComponentUpdateView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for ComponentUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated component {} to revision {}",
            format_message_highlight(&self.0.component_name),
            format_message_highlight(&self.0.component_revision),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        component_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentGetView(pub ComponentView);

impl Masked for ComponentGetView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for ComponentGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for component {}",
            format_message_highlight(&self.0.component_name)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        component_view_fields(&self.0)
    }
}

impl StructuredOutput for ComponentGetView {
    const KIND: &'static str = "component.get";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentManifestTraceView {
    pub component_name: ComponentName,
    pub properties: ComponentLayerProperties,
}

impl StructuredOutput for ComponentManifestTraceView {
    const KIND: &'static str = "component.manifest-trace";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_masked_value(config)
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

impl TextOutput for ComponentManifestTraceView {
    fn log(&self) {
        log_manifest_trace_properties(&self.properties);
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        if config.show_secrets {
            self.log();
        } else {
            let mut properties = serde_json::to_value(&self.properties)?;
            mask_component_layer_properties(&mut properties);
            log_manifest_trace_value(&properties);
        }
        Ok(())
    }
}

impl ComponentManifestTraceView {
    fn to_masked_value(&self, config: MaskingConfig) -> anyhow::Result<Value> {
        let mut value = serde_json::to_value(self)?;
        if !config.show_secrets
            && let Some(properties) = value
                .as_object_mut()
                .and_then(|object| object.get_mut("properties"))
        {
            mask_component_layer_properties(properties);
        }
        Ok(value)
    }
}

fn log_manifest_trace_properties(properties: &ComponentLayerProperties) {
    let rendered = if SHOULD_COLORIZE.should_colorize() {
        to_colored_json(properties)
    } else {
        serde_json::to_string_pretty(properties).map_err(Into::into)
    };

    log_manifest_trace_rendered(rendered);
}

fn log_manifest_trace_value(properties: &Value) {
    let rendered = if SHOULD_COLORIZE.should_colorize() {
        to_colored_json(properties)
    } else {
        serde_json::to_string_pretty(properties).map_err(Into::into)
    };

    log_manifest_trace_rendered(rendered);
}

fn log_manifest_trace_rendered(rendered: anyhow::Result<String>) {
    match rendered {
        Ok(rendered) => {
            for line in rendered.lines() {
                logln(line);
            }
        }
        Err(error) => logln(format!("<failed to render manifest trace: {error:#}>")),
    }
}

fn mask_component_layer_properties(properties: &mut Value) {
    let Some(properties) = properties.as_object_mut() else {
        return;
    };

    if let Some(config) = properties.get_mut("config") {
        mask_config_property_payloads(config);
    }
    if let Some(env) = properties.get_mut("env") {
        mask_sensitive_keyed_values(env);
    }
    if let Some(plugins) = properties.get_mut("plugins") {
        mask_sensitive_keyed_values(plugins);
    }
}

fn mask_config_property_payloads(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                match key.as_str() {
                    "value" | "newValue" => mask_json_leaf_values(value),
                    "insertedEntries" | "updatedEntries" => mask_json_object_values(value),
                    _ => mask_config_property_payloads(value),
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                mask_config_property_payloads(value);
            }
        }
        _ => {}
    }
}

fn mask_json_object_values(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for value in object.values_mut() {
            mask_json_leaf_values(value);
        }
    }
}

fn mask_json_leaf_values(value: &mut Value) {
    match value {
        Value::Null => {}
        Value::Array(values) => {
            for value in values {
                mask_json_leaf_values(value);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                mask_json_leaf_values(value);
            }
        }
        _ => *value = Value::String(mask_secret()),
    }
}

fn mask_sensitive_keyed_values(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if is_sensitive_key(key) {
                    mask_json_leaf_values(value);
                } else {
                    mask_sensitive_keyed_values(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                mask_sensitive_keyed_values(value);
            }
        }
        _ => {}
    }
}

fn format_files(files: &[InitialAgentFile]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{} {} {}",
                file.permissions.as_compact_str(),
                file.path.as_path().as_str().log_color_highlight(),
                file.content_hash.0.to_string().black()
            )
        })
        .join("\n")
}

fn format_plugins(plugins: &[InstalledPlugin]) -> String {
    plugins
        .iter()
        .map(|plugin| {
            let plugin_id = format!(
                "{}: {}/{}",
                plugin.priority,
                plugin.plugin_name.log_color_highlight(),
                plugin.plugin_version.log_color_highlight(),
            );

            if plugin.parameters.is_empty() {
                plugin_id
            } else {
                format!(
                    "{}:\n{}",
                    plugin_id,
                    plugin
                        .parameters
                        .iter()
                        .map(|(k, v)| format!("  {}={}", k, v))
                        .join("\n")
                )
            }
        })
        .join("\n")
}

fn format_typed_config(config: &[TypedAgentConfigEntry]) -> String {
    config
        .iter()
        .map(|entry| {
            let key = entry.path.join(".");
            let value = golem_common::schema::render::to_json_value(
                entry.value.graph(),
                entry.value.root_type(),
                entry.value.value(),
            )
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "<invalid>".to_string());
            format!("{}={}", key.log_color_highlight(), value)
        })
        .join("\n")
}

pub fn format_component_applied_layers(
    applied_layers: &[(ComponentLayerId, Option<String>)],
) -> String {
    applied_layers
        .iter()
        .map(|(id, selection)| match selection {
            Some(selection) => {
                format!("{}[{}]", id.name(), selection.as_str())
            }
            None => id.name().to_string(),
        })
        .join(", ")
}
