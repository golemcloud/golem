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

use crate::model::component::ComponentView;
use crate::model::text::fmt::*;
use crate::model::ComponentName;
use cli_table::{format::Justify, Table};

use serde::{Deserialize, Serialize};

#[derive(Table)]
struct ComponentTableView {
    #[table(title = "Name")]
    pub component_name: ComponentName,
    #[table(title = "Version", justify = "Justify::Right")]
    pub component_version: u64,
    #[table(title = "Size", justify = "Justify::Right")]
    pub component_size: u64,
    #[table(title = "Exports count", justify = "Justify::Right")]
    pub n_exports: usize,
}

impl From<&ComponentView> for ComponentTableView {
    fn from(value: &ComponentView) -> Self {
        Self {
            component_name: value.component_name.clone(),
            component_version: value.component_version,
            component_size: value.component_size,
            n_exports: value.exports.len(),
        }
    }
}

impl TextView for Vec<ComponentView> {
    fn log(&self) {
        log_table::<_, ComponentTableView>(self.as_slice())
    }
}

fn component_view_fields(view: &ComponentView) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Component name", &view.component_name, format_main_id)
        .fmt_field("Component ID", &view.component_id, format_id)
        .fmt_field("Component type", &view.component_type, |t| t.to_string())
        .fmt_field("Component version", &view.component_version, format_id)
        .fmt_field_option("Project ID", &view.project_id, format_id)
        .fmt_field("Component size", &view.component_size, format_binary_size)
        .fmt_field_option("Created at", &view.created_at, |d| d.to_string())
        .fmt_field_optional("Environment", &view.env, !&view.env.is_empty(), |env| {
            format_env(view.show_sensitive, env)
        })
        .fmt_field("Exports", &view.exports, |e| format_exports(e.as_slice()))
        .fmt_field_optional(
            "Dynamic WASM RPC links",
            &view.dynamic_linking,
            !view.dynamic_linking.is_empty(),
            format_dynamic_links,
        )
        .fmt_field_optional(
            "Initial file system",
            view.files.as_slice(),
            !view.files.is_empty(),
            format_ifs_entry,
        );

    fields.build()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentCreateView(pub ComponentView);

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

impl MessageWithFields for ComponentUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated component {} to version {}",
            format_message_highlight(&self.0.component_name),
            format_message_highlight(&self.0.component_version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        component_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentGetView(pub ComponentView);

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentReplStartedView(pub ComponentView);

impl MessageWithFields for ComponentReplStartedView {
    fn message(&self) -> String {
        format!(
            "Started Rib REPL for component {} using version {}",
            format_message_highlight(&self.0.component_name),
            format_message_highlight(&self.0.component_version),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        component_view_fields(&self.0)
    }
}

const SENSITIVE_ENV_VAR_NAME_PATTERNS: &[&str] = &[
    "CREDENTIAL",
    "CREDENTIALS",
    "KEY",
    "PASS",
    "PASSWORD",
    "PWD",
    "SECRET",
    "TOKEN",
];

// NOTE: Keys are expected to be already uppercase
pub fn is_sensitive_env_var_name(show_sensitive: bool, name: &str) -> bool {
    if show_sensitive {
        false
    } else {
        SENSITIVE_ENV_VAR_NAME_PATTERNS
            .iter()
            .any(|pattern| name.contains(pattern))
    }
}
