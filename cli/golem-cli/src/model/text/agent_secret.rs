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

use golem_common::model::agent_secret::AgentSecretDto;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretCreateView {
    pub secret: AgentSecretDto,
    #[serde(skip)]
    pub show_sensitive: bool,
}

impl MessageWithFields for AgentSecretCreateView {
    fn message(&self) -> String {
        format!(
            "Created a new agent secret {}",
            format_message_highlight(&self.secret.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.secret, self.show_sensitive)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretGetView {
    pub secret: AgentSecretDto,
    #[serde(skip)]
    pub show_sensitive: bool,
}

impl MessageWithFields for AgentSecretGetView {
    fn message(&self) -> String {
        format!(
            "Agent secret {}",
            format_message_highlight(&self.secret.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.secret, self.show_sensitive)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretUpdateView {
    pub secret: AgentSecretDto,
    #[serde(skip)]
    pub show_sensitive: bool,
}

impl MessageWithFields for AgentSecretUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated agent secret {}",
            format_message_highlight(&self.secret.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.secret, self.show_sensitive)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretDeleteView {
    pub secret: AgentSecretDto,
    #[serde(skip)]
    pub show_sensitive: bool,
}

impl MessageWithFields for AgentSecretDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted agent secret {}",
            format_message_highlight(&self.secret.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        agent_secret_view_fields(&self.secret, self.show_sensitive)
    }
}

fn agent_secret_view_fields(view: &AgentSecretDto, show_sensitive: bool) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Environment ID", &view.environment_id.0, format_main_id)
        .fmt_field("Path", &view.path, format_main_id)
        .fmt_field("ID", &view.id, format_id)
        .fmt_field("Revision", &view.revision.get(), format_id)
        // TODO: better analysed type rendering
        .fmt_field("Secret Type", &view.secret_type, |st| format!("{:?}", st))
        .fmt_field_option(
            "Secret Value",
            &view.secret_value,
            |v| {
                if show_sensitive {
                    v.to_string()
                } else {
                    "***".to_string()
                }
            },
        );

    fields.build()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretListView {
    pub secrets: Vec<AgentSecretDto>,
    #[serde(skip)]
    pub show_sensitive: bool,
}

impl TextView for AgentSecretListView {
    fn log(&self) {
        let mut table = new_table(vec![
            Column::new("Environment ID").fixed(),
            Column::new("Path"),
            Column::new("ID").fixed(),
            Column::new("Revision").fixed_right(),
            Column::new("Secret Type").fixed(),
            Column::new("Secret Value"),
        ]);
        for secret in &self.secrets {
            let secret_value = if self.show_sensitive {
                secret
                    .secret_value
                    .as_ref()
                    .unwrap_or(&serde_json::Value::Null)
                    .to_string()
            } else {
                "***".to_string()
            };
            table.add_row(vec![
                secret.environment_id.0.to_string(),
                secret.path.to_string(),
                secret.id.to_string(),
                secret.revision.get().to_string(),
                format!("{:?}", secret.secret_type),
                secret_value,
            ]);
        }
        log_table(table);
    }
}
