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
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::worker::RawAgentId;
use chrono::{DateTime, Utc};
use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
use golem_common::model::agent::{AgentConfigSource, AgentTypeName};
use golem_common::model::component::{
    AgentConfigEntryDto, ComponentDto, ComponentId, ComponentRevision,
};
use golem_common::model::component::{
    AgentFileOptions, AgentFilePath, AgentTypeProvisionConfigCreation, ArchiveFilePath,
    PluginInstallation,
};
use golem_common::model::component::{AgentFilePermissions, ComponentName};
use golem_common::schema::agent::{AgentTypeSchema, FieldSource, InputSchema, OutputSchema};
use golem_common::schema::graph::SchemaGraph;

use crate::agent_id_display::render_type_for_language;
use crate::model::app_raw;
use crate::model::masking::{
    Masked, MaskingConfig, mask_sensitive_map, mask_typed_agent_config_entries,
};
use golem_common::model::environment::EnvironmentId;
use heck::{ToLowerCamelCase, ToSnakeCase};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub enum ComponentRevisionSelection<'a> {
    ByAgentName(&'a RawAgentId),
    ByExplicitRevision(ComponentRevision),
}

impl<'a> From<&'a RawAgentId> for ComponentRevisionSelection<'a> {
    fn from(value: &'a RawAgentId) -> Self {
        Self::ByAgentName(value)
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
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentTypeManifestProvisionConfig {
    pub env: BTreeMap<String, String>,
    pub config: Vec<AgentConfigEntryDto>,
    pub files_source: PathBuf,
    pub files: Vec<app_raw::InitialComponentFile>,
    pub plugins: Vec<app_raw::PluginInstallation>,
}

impl AgentTypeManifestProvisionConfig {
    pub fn to_provision_config_creation(
        &self,
        resolved_plugins: Vec<PluginInstallation>,
    ) -> AgentTypeProvisionConfigCreation {
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
        AgentTypeProvisionConfigCreation {
            env: self.env.clone(),
            config: self.config.clone(),
            files,
            plugin_installations: resolved_plugins,
        }
    }
}

#[derive(Debug)]
pub struct ComponentDeployProperties {
    pub wasm_path: PathBuf,
    pub agent_types: Vec<AgentTypeSchema>,
    pub agent_type_configs: BTreeMap<AgentTypeName, AgentTypeManifestProvisionConfig>,
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
    let agent_name = if wrapper_naming {
        format!("{}.", agent.type_name.0)
    } else {
        "  ".to_string()
    };
    for method in &agent.methods {
        let output = render_output_schema(&agent.schema, &method.output_schema, &lang);
        let input = render_input_schema(&agent.schema, &method.input_schema, &lang, true);
        if output.is_empty() {
            result.push(format!("{}{}({})", agent_name, method.name, input));
        } else {
            result.push(format!(
                "{}{}({}) -> {}",
                agent_name, method.name, input, output
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
