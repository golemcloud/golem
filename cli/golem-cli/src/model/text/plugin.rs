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
use golem_common::model::component::PluginInstallation;
use golem_common::model::plugin_registration::{
    ComponentTransformerPluginSpec, OplogProcessorPluginSpec, PluginRegistrationDto, PluginSpecDto,
};
use itertools::Itertools;

#[derive(Table)]
struct PluginRegistrationTableView {
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
}

impl From<&PluginRegistrationDto> for PluginRegistrationTableView {
    fn from(value: &PluginRegistrationDto) -> Self {
        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            description: value.description.clone(),
            homepage: value.homepage.clone(),
            typ: value.typ(),
        }
    }
}

impl TextView for Vec<PluginRegistrationDto> {
    fn log(&self) {
        log_table::<_, PluginRegistrationTableView>(self.as_slice())
    }
}

impl MessageWithFields for PluginRegistrationDto {
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
            .field("Type", &self.typ())
            .fmt_field_option(
                "Validate URL",
                &self.component_transformer_validate_url(),
                |u| u.to_string(),
            )
            .fmt_field_option(
                "Transform URL",
                &self.component_transformer_validate_url(),
                |u| u.to_string(),
            )
            .fmt_field_option(
                "Component ID",
                &self.oplog_processor_component_id(),
                format_id,
            )
            .fmt_field_option(
                "Component Version",
                &self.oplog_processor_component_revision(),
                format_id,
            );

        fields.build()
    }
}

// TODO: atomic
/*impl MessageWithFields for PluginInstallation {
    fn message(&self) -> String {
        format!(
            "Installed plugin {} version {}",
            format_message_highlight(&self.environment_plugin_grant_id),
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
    #[table(title = "Parameters")]
    pub parameters: String,
}

impl From<&PluginRegistrationDto> for PluginInstallationTableView {
    fn from(value: &PluginRegistrationDto) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.clone(),
            version: value.version.clone(),
            parameters: value
                .parameters
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .join(", "),
        }
    }
}

impl TextView for Vec<PluginRegistrationDto> {
    fn log(&self) {
        log_table::<_, PluginInstallationTableView>(self.as_slice())
    }
}*/
