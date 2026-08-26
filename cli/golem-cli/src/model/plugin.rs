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

use crate::model::cli_output::StructuredOutput;
use crate::model::masking::Masked;
use crate::model::text_format::{
    Column, FieldsBuilder, MessageWithFields, TextOutput, format_id, format_main_id,
    format_message_highlight, log_table, new_table_full_condensed,
};
use golem_common::model::component::ComponentRevision;
use golem_common::model::plugin_registration::PluginRegistrationDto;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginTypeSpecificManifest {
    OplogProcessor(OplogProcessorManifest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OplogProcessorManifest {
    pub component_id: Uuid,
    pub component_revision: ComponentRevision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: PathBuf,
    pub homepage: String,
    pub specs: PluginTypeSpecificManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginNameAndVersion {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginSource {
    Own,
    Builtin,
    Shared,
}

impl std::fmt::Display for PluginSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginSource::Own => write!(f, "own"),
            PluginSource::Builtin => write!(f, "builtin"),
            PluginSource::Shared => write!(f, "shared"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListEntry {
    pub plugin: PluginRegistrationDto,
    pub source: PluginSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListView {
    pub plugins: Vec<PluginListEntry>,
}

impl StructuredOutput for PluginListView {
    const KIND: &'static str = "plugin.list";
}

impl TextOutput for PluginListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Plugin name").fixed(),
            Column::new("Plugin version").fixed(),
            Column::new("Source").fixed(),
            Column::new("Type").fixed(),
            Column::new("Description"),
            Column::new("Homepage"),
        ]);
        for entry in &self.plugins {
            table.add_row(vec![
                entry.plugin.name.clone(),
                entry.plugin.version.clone(),
                entry.source.to_string(),
                entry.plugin.typ_as_str().to_string(),
                entry.plugin.description.clone(),
                entry.plugin.homepage.clone(),
            ]);
        }
        log_table(table);
    }
}

impl TextOutput for Vec<PluginRegistrationDto> {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Plugin name").fixed(),
            Column::new("Plugin version").fixed(),
            Column::new("Type").fixed(),
            Column::new("Description"),
            Column::new("Homepage"),
        ]);
        for plugin in self {
            table.add_row(vec![
                plugin.name.clone(),
                plugin.version.clone(),
                plugin.typ_as_str().to_string(),
                plugin.description.clone(),
                plugin.homepage.clone(),
            ]);
        }
        log_table(table);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRegistrationRegisterView(pub PluginRegistrationDto);

impl Masked for PluginRegistrationRegisterView {}

impl MessageWithFields for PluginRegistrationRegisterView {
    fn message(&self) -> String {
        format!(
            "Registered new plugin {} version {}",
            format_message_highlight(&self.0.name),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        plugin_registration_fields(&self.0)
    }
}

impl StructuredOutput for PluginRegistrationRegisterView {
    const KIND: &'static str = "plugin.register";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRegistrationGetView(pub PluginRegistrationDto);

impl Masked for PluginRegistrationGetView {}

impl MessageWithFields for PluginRegistrationGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for plugin {} version {}",
            format_message_highlight(&self.0.name),
            format_message_highlight(&self.0.version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        plugin_registration_fields(&self.0)
    }
}

impl StructuredOutput for PluginRegistrationGetView {
    const KIND: &'static str = "plugin.get";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginUnregisterView {
    pub unregistered: bool,
    pub plugin_id: Uuid,
    pub name: String,
    pub version: String,
}

impl Masked for PluginUnregisterView {}

impl MessageWithFields for PluginUnregisterView {
    fn message(&self) -> String {
        format!(
            "Unregistered plugin {}",
            format_message_highlight(&self.name)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        fields
            .fmt_field("Name", &self.name, format_main_id)
            .fmt_field("Version", &self.version, format_main_id)
            .fmt_field("Plugin ID", &self.plugin_id, format_id);
        fields.build()
    }
}

impl StructuredOutput for PluginUnregisterView {
    const KIND: &'static str = "plugin.unregister";
}

fn plugin_registration_fields(plugin: &PluginRegistrationDto) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Name", &plugin.name, format_main_id)
        .fmt_field("Version", &plugin.version, format_main_id)
        .field("Description", &plugin.description)
        .field("Homepage", &plugin.homepage)
        .field("Type", &plugin.typ_as_str())
        .fmt_field_option(
            "Component ID",
            &plugin.oplog_processor_component_id(),
            format_id,
        )
        .fmt_field_option(
            "Component Version",
            &plugin.oplog_processor_component_revision(),
            format_id,
        );

    fields.build()
}
