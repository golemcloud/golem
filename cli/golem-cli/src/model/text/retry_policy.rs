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
use golem_common::model::retry_policy::RetryPolicyDto;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicyCreateView(pub RetryPolicyDto);

impl MessageWithFields for RetryPolicyCreateView {
    fn message(&self) -> String {
        format!(
            "Created retry policy {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        retry_policy_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicyGetView(pub RetryPolicyDto);

impl MessageWithFields for RetryPolicyGetView {
    fn message(&self) -> String {
        format!("Retry policy {}", format_message_highlight(&self.0.name),)
    }

    fn fields(&self) -> Vec<(String, String)> {
        retry_policy_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicyUpdateView(pub RetryPolicyDto);

impl MessageWithFields for RetryPolicyUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated retry policy {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        retry_policy_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicyDeleteView(pub RetryPolicyDto);

impl MessageWithFields for RetryPolicyDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted retry policy {}",
            format_message_highlight(&self.0.name),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        retry_policy_view_fields(&self.0)
    }
}

fn retry_policy_view_fields(view: &RetryPolicyDto) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Environment ID", &view.environment_id.0, format_main_id)
        .fmt_field("Name", &view.name, format_main_id)
        .fmt_field("ID", &view.id, format_id)
        .fmt_field("Revision", &view.revision.get(), format_id)
        .fmt_field("Priority", &view.priority, ToString::to_string)
        .fmt_field("Predicate", &view.predicate_json, ToString::to_string)
        .fmt_field("Policy", &view.policy_json, ToString::to_string);

    fields.build()
}

#[derive(Table)]
struct RetryPolicyTableView {
    #[table(title = "EnvironmentId")]
    pub environment_id: String,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Revision", Justify = "Right")]
    pub revision: u64,
    #[table(title = "Priority", Justify = "Right")]
    pub priority: u32,
}

impl From<&RetryPolicyDto> for RetryPolicyTableView {
    fn from(value: &RetryPolicyDto) -> Self {
        Self {
            environment_id: value.environment_id.0.to_string(),
            name: value.name.clone(),
            id: value.id.to_string(),
            revision: value.revision.get(),
            priority: value.priority,
        }
    }
}

impl TextView for Vec<RetryPolicyDto> {
    fn log(&self) {
        log_table::<_, RetryPolicyTableView>(self.as_slice())
    }
}
