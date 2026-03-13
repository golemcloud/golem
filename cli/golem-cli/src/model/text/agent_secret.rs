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
use golem_common::model::agent_secret::AgentSecretDto;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretCreateView(pub AgentSecretDto);

impl MessageWithFields for AgentSecretCreateView {
    fn message(&self) -> String {
        format!(
            "Created a new agent secret {}",
            format_message_highlight(&self.0.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretUpdateView(pub AgentSecretDto);

impl MessageWithFields for AgentSecretUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated agent secret {}",
            format_message_highlight(&self.0.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretDeleteView(pub AgentSecretDto);

impl MessageWithFields for AgentSecretDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted agent secret {}",
            format_message_highlight(&self.0.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.0)
    }
}

fn agent_secret_view_fields(view: &AgentSecretDto) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Environment ID", &view.environment_id.0, format_main_id)
        .fmt_field("Path", &view.path, format_main_id)
        .fmt_field("ID", &view.id, format_id)
        .fmt_field("Revision", &view.revision.get(), format_id)
        // TODO: better analysed type rendering
        .fmt_field("Secret Type", &view.secret_type, |st| format!("{:?}", st))
        .fmt_field_option("Secret Value", &view.secret_value, ToString::to_string);

    fields.build()
}

#[derive(Table)]
struct AgentSecretTableView {
    #[table(title = "EnvironmentId")]
    pub environment_id: String,
    #[table(title = "Path")]
    pub path: String,
    #[table(title = "ID")]
    pub id: String,
    #[table(title = "Revision", Justify = "Right")]
    pub revision: u64,
    #[table(title = "Secret Type")]
    pub secret_type: String,
    #[table(title = "Secret Value")]
    pub secret_value: String,
}

impl From<&AgentSecretDto> for AgentSecretTableView {
    fn from(value: &AgentSecretDto) -> Self {
        Self {
            environment_id: value.environment_id.0.to_string(),
            path: value.path.to_string(),
            id: value.id.to_string(),
            revision: value.revision.get(),
            secret_type: format!("{:?}", value.secret_type),
            secret_value: value
                .secret_value
                .as_ref()
                .unwrap_or(&serde_json::Value::Null)
                .to_string(),
        }
    }
}

impl TextView for Vec<AgentSecretDto> {
    fn log(&self) {
        log_table::<_, AgentSecretTableView>(self.as_slice())
    }
}
