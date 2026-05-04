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
        .fmt_field(
            "Predicate",
            &serde_json::to_string(&view.predicate).expect("Failed to serialize Retry predicate"),
            ToString::to_string,
        )
        .fmt_field(
            "Policy",
            &serde_json::to_string(&view.policy).expect("Failed to serialize Retry policy"),
            ToString::to_string,
        );

    fields.build()
}

impl TextView for Vec<RetryPolicyDto> {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Environment ID").fixed(),
            Column::new("Name").fixed(),
            Column::new("ID").fixed(),
            Column::new("Revision").fixed_right(),
            Column::new("Priority").fixed_right(),
        ]);

        for policy in self {
            table.add_row(vec![
                policy.environment_id.0.to_string(),
                policy.name.clone(),
                policy.id.to_string(),
                policy.revision.get().to_string(),
                policy.priority.to_string(),
            ]);
        }

        log_table(table)
    }
}
