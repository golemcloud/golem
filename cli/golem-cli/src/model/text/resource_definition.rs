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
        .field("Enforcement", &r.enforcement_action)
        .field("Unit", &r.unit)
        .field("Units", &r.units);

    fields.build()
}

impl TextView for Vec<ResourceDefinition> {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Name"),
            Column::new("Revision").fixed_right(),
            Column::new("Limit").fixed_right(),
            Column::new("Enforcement Action").fixed_right(),
            Column::new("Unit").fixed_right(),
            Column::new("Units").fixed_right(),
        ]);
        for row in self {
            table.add_row(vec![
                row.name.to_string(),
                row.revision.to_string(),
                serde_json::to_string(&row.limit).unwrap_or_else(|_| format!("{:?}", row.limit)),
                row.enforcement_action.to_string(),
                row.unit.to_string(),
                row.units.to_string(),
            ]);
        }
        log_table(table);
    }
}
