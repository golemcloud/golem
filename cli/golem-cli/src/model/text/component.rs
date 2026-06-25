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

use crate::model::app::ComponentLayerProperties;
use crate::model::cli_output::StructuredOutput;
use crate::model::component::ComponentView;
use crate::model::masking::{Masked, MaskingConfig, is_sensitive_key, mask_secret};
use crate::model::text::fmt::*;
use colored::control::SHOULD_COLORIZE;
use golem_common::model::card::{PolymorphicCard, render_polymorphic_permission};
use golem_common::model::component::ComponentName;
use serde::Serializer;
use serde::ser::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
            render_polymorphic_permission(permission)
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
