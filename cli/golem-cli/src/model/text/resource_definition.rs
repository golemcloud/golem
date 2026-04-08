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

use crate::model::text::fmt::*;
use cli_table::Table;
use golem_common::model::quota::ResourceDefinition;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinitionCreateView(pub ResourceDefinition);

impl MessageWithFields for ResourceDefinitionCreateView {
    fn message(&self) -> String {
        format!(
            "Created resource definition {}",
            format_message_highlight(&self.0.name.0),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        resource_definition_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinitionUpdateView(pub ResourceDefinition);

impl MessageWithFields for ResourceDefinitionUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated resource definition {}",
            format_message_highlight(&self.0.name.0),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        resource_definition_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinitionDeleteView(pub ResourceDefinition);

impl MessageWithFields for ResourceDefinitionDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted resource definition {}",
            format_message_highlight(&self.0.name.0),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        resource_definition_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinitionGetView(pub ResourceDefinition);

impl MessageWithFields for ResourceDefinitionGetView {
    fn message(&self) -> String {
        format!(
            "Resource definition {}",
            format_message_highlight(&self.0.name.0),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        resource_definition_fields(&self.0)
    }
}

fn resource_definition_fields(r: &ResourceDefinition) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Environment ID", &r.environment_id.0, format_main_id)
        .fmt_field("Name", &r.name.0, format_main_id)
        .fmt_field("ID", &r.id, format_id)
        .fmt_field("Revision", &r.revision.get(), format_id)
        .fmt_field("Limit", &r.limit, |l| {
            serde_json::to_string(l).unwrap_or_else(|_| format!("{:?}", l))
        })
        .fmt_field("Enforcement", &r.enforcement_action, |a| {
            serde_json::to_string(a)
                .unwrap_or_else(|_| format!("{:?}", a))
                .trim_matches('"')
                .to_string()
        })
        .fmt_field("Unit", &r.unit, ToString::to_string)
        .fmt_field("Units", &r.units, ToString::to_string);

    fields.build()
}

#[derive(Table)]
struct ResourceDefinitionTableView {
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Revision", Justify = "Right")]
    pub revision: u64,
    #[table(title = "Limit")]
    pub limit: String,
    #[table(title = "Enforcement")]
    pub enforcement_action: String,
    #[table(title = "Unit")]
    pub unit: String,
}

impl From<&ResourceDefinition> for ResourceDefinitionTableView {
    fn from(r: &ResourceDefinition) -> Self {
        Self {
            name: r.name.0.clone(),
            id: r.id.to_string(),
            revision: r.revision.get(),
            limit: serde_json::to_string(&r.limit).unwrap_or_else(|_| format!("{:?}", r.limit)),
            enforcement_action: serde_json::to_string(&r.enforcement_action)
                .unwrap_or_else(|_| format!("{:?}", r.enforcement_action))
                .trim_matches('"')
                .to_string(),
            unit: r.unit.clone(),
        }
    }
}

impl TextView for Vec<ResourceDefinition> {
    fn log(&self) {
        log_table::<_, ResourceDefinitionTableView>(self.as_slice())
    }
}
