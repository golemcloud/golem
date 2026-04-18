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

use crate::agent_id_display::{SourceLanguage, render_type_for_language};
use crate::model::text::fmt::*;

use comfy_table::Cell;
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
        format!("Agent secret {}", format_message_highlight(&self.secret.id),)
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
        .fmt_field("Secret Type", &view.secret_type, |st| {
            render_type_for_language(&SourceLanguage::default(), st, false)
        })
        .fmt_field_option("Secret Value", &view.secret_value, |v| {
            if show_sensitive {
                v.to_string()
            } else {
                "***".to_string()
            }
        });

    fields.build()
}

fn format_secret_value(show_sensitive: bool, secret_value: &Option<serde_json::Value>) -> String {
    if show_sensitive {
        secret_value
            .as_ref()
            .unwrap_or(&serde_json::Value::Null)
            .to_string()
    } else {
        "***".to_string()
    }
}

fn wrap_uuid_for_table(uuid: &str) -> String {
    let split_at = uuid
        .match_indices('-')
        .nth(2)
        .map(|(index, _)| index + 1)
        .unwrap_or(uuid.len() / 2);

    format!("{}\n{}", &uuid[..split_at], &uuid[split_at..])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSecretListView {
    pub secrets: Vec<AgentSecretDto>,
    #[serde(skip)]
    pub show_sensitive: bool,
    #[serde(skip)]
    pub environment_name: String,
    #[serde(skip)]
    pub show_ids: bool,
}

impl AgentSecretListView {
    fn table(&self) -> ComfyTable {
        let mut table = new_table(vec![
            Column::new("Environment").fixed(),
            Column::new("Path"),
            Column::new("Revision").fixed_right(),
            Column::new("Secret Value"),
        ]);

        if self.show_ids {
            table.set_header(vec![
                Cell::new("Environment"),
                Cell::new("Path"),
                Cell::new("Revision"),
                Cell::new("Secret Value"),
                Cell::new("Environment ID"),
                Cell::new("Secret ID"),
            ]);
        }

        for secret in &self.secrets {
            let secret_value = format_secret_value(self.show_sensitive, &secret.secret_value);

            if self.show_ids {
                table.add_row(vec![
                    self.environment_name.clone(),
                    secret.path.to_string(),
                    secret.revision.get().to_string(),
                    secret_value,
                    wrap_uuid_for_table(&secret.environment_id.0.to_string()),
                    wrap_uuid_for_table(&secret.id.to_string()),
                ]);
            } else {
                table.add_row(vec![
                    self.environment_name.clone(),
                    secret.path.to_string(),
                    secret.revision.get().to_string(),
                    secret_value,
                ]);
            }
        }

        table
    }
}

impl TextView for AgentSecretListView {
    fn log(&self) {
        let table = self.table();
        log_table(table);
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentSecretListView, wrap_uuid_for_table};
    use golem_common::model::agent_secret::{
        AgentSecretDto, AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_wasm::analysis::analysed_type::str;
    use serde_json::json;
    use test_r::test;

    fn sample_secret() -> AgentSecretDto {
        AgentSecretDto {
            id: "00000000-0000-0000-0000-000000000002"
                .parse::<AgentSecretId>()
                .unwrap(),
            environment_id: "00000000-0000-0000-0000-000000000001"
                .parse::<EnvironmentId>()
                .unwrap(),
            path: CanonicalAgentSecretPath(vec!["token".to_string()]),
            revision: AgentSecretRevision::new(7).unwrap(),
            secret_type: str(),
            secret_value: Some(json!("***")),
        }
    }

    #[test]
    fn wrapped_uuid_splits_across_lines() {
        let wrapped = wrap_uuid_for_table("00000000-0000-0000-0000-000000000001");

        assert!(wrapped.contains('\n'));
        assert!(wrapped.starts_with("00000000-0000-0000-"));
        assert!(wrapped.ends_with("0000-000000000001"));
    }

    #[test]
    fn list_view_json_stays_stable() {
        let view = AgentSecretListView {
            secrets: vec![sample_secret()],
            show_sensitive: true,
            environment_name: "local".to_string(),
            show_ids: true,
        };

        let json = serde_json::to_value(&view).unwrap();

        assert!(json.get("secrets").is_some());
        assert!(json.get("environment_name").is_none());
        assert!(json.get("show_ids").is_none());
        assert!(json.get("show_sensitive").is_none());
    }

    #[test]
    fn default_table_omits_ids_and_type_columns() {
        let view = AgentSecretListView {
            secrets: vec![sample_secret()],
            show_sensitive: false,
            environment_name: "local".to_string(),
            show_ids: false,
        };

        let rendered = view.table().to_string();

        assert!(rendered.contains("Environment"));
        assert!(rendered.contains("Path"));
        assert!(rendered.contains("Revision"));
        assert!(rendered.contains("Secret Value"));
        assert!(!rendered.contains("Environment ID"));
        assert!(!rendered.contains("Secret ID"));
        assert!(!rendered.contains("Secret Type"));
        assert!(rendered.contains("local"));
        assert!(rendered.contains("token"));
        assert!(rendered.contains("***"));
    }

    #[test]
    fn ids_table_includes_wrapped_id_columns() {
        let view = AgentSecretListView {
            secrets: vec![sample_secret()],
            show_sensitive: false,
            environment_name: "local".to_string(),
            show_ids: true,
        };

        let rendered = view.table().to_string();

        assert!(rendered.contains("local"));
        assert!(rendered.contains("token"));
        assert!(!rendered.contains("Secret Type"));
        assert!(!rendered.is_empty());
    }
}
