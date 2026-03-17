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
use golem_common::model::agent::{
    AgentType, ComponentModelElementSchema, DataSchema, ElementSchema,
};
use golem_common::model::component::{
    AgentConfigEntry, ComponentDto, ComponentId, ComponentRevision, InstalledPlugin,
};
use golem_common::model::component::{ComponentName, InitialComponentFile};

use crate::agent_id_display::render_type_for_language;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::trim_date::TrimDateTime;
use heck::{ToLowerCamelCase, ToSnakeCase};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    #[serde(skip)]
    pub show_sensitive: bool,
    #[serde(skip)]
    pub show_exports_for_rib: bool,

    pub component_name: ComponentName,
    pub component_id: ComponentId,
    pub component_version: Option<String>,
    pub component_revision: u64,
    pub component_size: u64,
    pub created_at: DateTime<Utc>,
    pub environment_id: EnvironmentId,
    pub exports: Vec<String>,
    pub agent_types: Vec<AgentType>,
    pub files: Vec<InitialComponentFile>,
    pub plugins: Vec<InstalledPlugin>,
    pub env: BTreeMap<String, String>,
}

impl ComponentView {
    pub fn new_rib_style(show_sensitive: bool, value: ComponentDto) -> Self {
        Self::new(show_sensitive, true, value)
    }

    pub fn new_wit_style(show_sensitive: bool, value: ComponentDto) -> Self {
        Self::new(show_sensitive, false, value)
    }

    pub fn new(show_sensitive: bool, show_exports_for_rib: bool, value: ComponentDto) -> Self {
        let exports = {
            let agent_types = value.metadata.agent_types().to_vec();

            show_exported_agents(&agent_types, true, true)
        };

        ComponentView {
            show_sensitive,
            show_exports_for_rib,
            component_name: value.component_name,
            component_id: value.id,
            component_version: value.metadata.root_package_version().clone(),
            component_revision: value.revision.into(),
            component_size: value.component_size,
            created_at: value.created_at,
            environment_id: value.environment_id,
            exports,
            agent_types: value.metadata.agent_types().to_vec(),
            files: value.files,
            plugins: value.installed_plugins,
            env: value.env,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComponentDeployProperties {
    pub wasm_path: PathBuf,
    pub agent_types: Vec<AgentType>,
    pub files: Vec<crate::model::app::InitialComponentFile>,
    pub plugins: Vec<crate::model::app::PluginInstallation>,
    pub env: BTreeMap<String, String>,
    pub config_vars: BTreeMap<String, String>,
    pub agent_config: Vec<AgentConfigEntry>,
}

impl TrimDateTime for ComponentView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            created_at: self.created_at.trim_date_time_ms(),
            ..self
        }
    }
}

pub fn show_exported_agents(
    agents: &[AgentType],
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> Vec<String> {
    agents
        .iter()
        .flat_map(|agent| render_exported_agent(agent, wrapper_naming, show_dummy_return_type))
        .collect()
}

pub fn show_exported_agent_constructors(agents: &[AgentType], wrapper_naming: bool) -> Vec<String> {
    agents
        .iter()
        .map(|c| render_agent_constructor(c, wrapper_naming, true))
        .collect()
}

fn render_exported_agent(
    agent: &AgentType,
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
        let output = render_data_schema(&method.output_schema, &lang, false);
        if output.is_empty() {
            result.push(format!(
                "{}{}({})",
                agent_name,
                method.name,
                render_data_schema(&method.input_schema, &lang, true),
            ));
        } else {
            result.push(format!(
                "{}{}({}) -> {}",
                agent_name,
                method.name,
                render_data_schema(&method.input_schema, &lang, true),
                output
            ));
        }
    }

    result
}

pub fn render_agent_constructor(
    agent: &AgentType,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> String {
    let lang = SourceLanguage::from(agent.source_language.as_str());
    render_agent_constructor_with_lang(agent, wrapper_naming, show_dummy_return_type, &lang)
}

fn render_agent_constructor_with_lang(
    agent: &AgentType,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
    lang: &SourceLanguage,
) -> String {
    let dummy_return_type = if show_dummy_return_type {
        " agent constructor"
    } else {
        ""
    };
    if wrapper_naming {
        format!(
            "{}({}){}",
            agent.type_name.0.clone(),
            render_data_schema(&agent.constructor.input_schema, lang, true),
            dummy_return_type
        )
    } else {
        format!(
            "{}({}){}",
            agent.type_name,
            render_data_schema(&agent.constructor.input_schema, lang, true),
            dummy_return_type
        )
    }
}

fn render_param_name(name: &str, lang: &SourceLanguage) -> String {
    match lang {
        SourceLanguage::Rust => name.to_snake_case(),
        SourceLanguage::TypeScript | SourceLanguage::Other(_) => name.to_lower_camel_case(),
    }
}

fn render_data_schema(
    schema: &DataSchema,
    lang: &SourceLanguage,
    show_param_names: bool,
) -> String {
    match schema {
        DataSchema::Tuple(elements) => elements
            .elements
            .iter()
            .map(|named_elem| {
                let rendered_type = render_element_schema(&named_elem.schema, lang);
                if show_param_names {
                    format!(
                        "{}: {}",
                        render_param_name(&named_elem.name, lang),
                        rendered_type
                    )
                } else {
                    rendered_type
                }
            })
            .join(", "),
        DataSchema::Multimodal(elements) => elements
            .elements
            .iter()
            .map(|named_elem| {
                format!(
                    "{}({})",
                    named_elem.name,
                    render_element_schema(&named_elem.schema, lang)
                )
            })
            .join(" | "),
    }
}

fn render_element_schema(schema: &ElementSchema, lang: &SourceLanguage) -> String {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            render_type_for_language(lang, element_type, true)
        }
        ElementSchema::UnstructuredText(text_descriptor) => {
            let mut result = "text".to_string();
            if let Some(restrictions) = &text_descriptor.restrictions {
                result.push('[');
                result.push_str(&restrictions.iter().map(|r| &r.language_code).join(", "));
                result.push(']');
            }
            result
        }
        ElementSchema::UnstructuredBinary(binary_descriptor) => {
            let mut result = "binary".to_string();
            if let Some(restrictions) = &binary_descriptor.restrictions {
                result.push('[');
                result.push_str(&restrictions.iter().map(|r| &r.mime_type).join(", "));
                result.push(']');
            }
            result
        }
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
