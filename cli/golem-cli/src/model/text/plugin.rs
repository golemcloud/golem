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

use crate::model::text::fmt::{
    format_id, format_main_id, format_message_highlight, log_table, FieldsBuilder,
    MessageWithFields, TextView,
};
use cli_table::Table;
use golem_client::model::PluginInstallation;

use crate::model::PluginDefinition;
use itertools::Itertools;

#[derive(Table)]
struct PluginDefinitionTableView {
    #[table(title = "Plugin name")]
    pub name: String,
    #[table(title = "Plugin version")]
    pub version: String,
    #[table(title = "Description")]
    pub description: String,
    #[table(title = "Homepage")]
    pub homepage: String,
    #[table(title = "Type")]
    pub typ: String,
    #[table(title = "Scope")]
    pub scope: String,
}

impl From<&PluginDefinition> for PluginDefinitionTableView {
    fn from(value: &PluginDefinition) -> Self {
        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            description: value.description.clone(),
            homepage: value.homepage.clone(),
            typ: value.typ.clone(),
            scope: value.scope.clone(),
        }
    }
}

impl TextView for Vec<PluginDefinition> {
    fn log(&self) {
        log_table::<_, PluginDefinitionTableView>(self.as_slice())
    }
}

impl MessageWithFields for PluginDefinition {
    fn message(&self) -> String {
        format!(
            "Got metadata for plugin {} version {}",
            format_message_highlight(&self.name),
            format_message_highlight(&self.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Name", &self.name, format_main_id)
            .fmt_field("Version", &self.version, format_main_id)
            .field("Description", &self.description)
            .field("Homepage", &self.homepage)
            .field("Scope", &self.scope)
            .field("Type", &self.typ)
            .fmt_field_option(
                "Validate URL",
                &self.component_transformer_validate_url,
                |f| f.to_string(),
            )
            .fmt_field_option(
                "Transform URL",
                &self.component_transformer_transform_url,
                |f| f.to_string(),
            )
            .fmt_field_option(
                "Component ID",
                &self.oplog_processor_component_id,
                format_id,
            )
            .fmt_field_option(
                "Component Version",
                &self.oplog_processor_component_version,
                format_id,
            );

        fields.build()
    }
}

impl MessageWithFields for PluginInstallation {
    fn message(&self) -> String {
        format!(
            "Installed plugin {} version {}",
            format_message_highlight(&self.plugin_name),
            format_message_highlight(&self.plugin_version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("ID", &self.id, format_main_id)
            .fmt_field("Plugin name", &self.plugin_version, format_id)
            .fmt_field("Plugin version", &self.plugin_version, format_id)
            .fmt_field("Priority", &self.priority, format_id);

        for (k, v) in &self.parameters {
            fields.fmt_field(k, v, format_id);
        }

        fields.build()
    }
}

// TODO: add component name to help with "multi-install"
#[derive(Table)]
struct PluginInstallationTableView {
    #[table(title = "Installation ID")]
    pub id: String,
    #[table(title = "Plugin name")]
    pub name: String,
    #[table(title = "Plugin version")]
    pub version: String,
    #[table(title = "Priority")]
    pub priority: String,
    #[table(title = "Parameters")]
    pub parameters: String,
}

impl From<&PluginInstallation> for PluginInstallationTableView {
    fn from(value: &PluginInstallation) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.plugin_name.clone(),
            version: value.plugin_version.clone(),
            priority: value.priority.to_string(),
            parameters: value
                .parameters
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .join(", "),
        }
    }
}

impl TextView for Vec<PluginInstallation> {
    fn log(&self) {
        log_table::<_, PluginInstallationTableView>(self.as_slice())
    }
}
