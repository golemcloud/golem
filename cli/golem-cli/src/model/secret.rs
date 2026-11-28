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
use crate::model::cli_output::StructuredOutput;
use crate::model::create_action::CreateAction;
use crate::model::masking::{Masked, MaskingConfig};
use crate::model::text_format::*;

use comfy_table::Cell;
use golem_client::model::AgentSecretDto;
use golem_common::schema::SchemaGraph;
use serde::Serialize as _;
use serde::Serializer;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretView {
    pub id: golem_common::model::agent_secret::AgentSecretId,
    pub environment_id: golem_common::model::environment::EnvironmentId,
    pub path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
    pub revision: golem_common::model::agent_secret::AgentSecretRevision,
    pub secret_type: SchemaGraph,
    pub has_value: bool,
    pub destinations: Vec<String>,
}

impl From<AgentSecretDto> for SecretView {
    fn from(value: AgentSecretDto) -> Self {
        Self {
            id: value.id,
            environment_id: value.environment_id,
            path: value.path,
            revision: value.revision,
            secret_type: value.secret_type,
            has_value: value.has_value,
            destinations: value.destinations,
        }
    }
}

impl Masked for SecretView {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretCreateView {
    pub action: CreateAction,
    #[serde(flatten)]
    pub secret: SecretView,
}

impl Masked for SecretCreateView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self {
            action: self.action,
            secret: self.secret.masked(config)?,
        })
    }
}

impl MessageWithFields for SecretCreateView {
    fn message(&self) -> String {
        let id = format_message_highlight(&self.secret.id);
        match self.action {
            CreateAction::Created => format!("Created a new secret {id}"),
            CreateAction::Updated => format!("Updated existing secret {id}"),
            CreateAction::Replaced => {
                format!("Replaced existing secret with a new secret {id}")
            }
        }
    }

    fn fields(&self) -> Vec<(String, String)> {
        secret_view_fields(&self.secret)
    }
}

impl StructuredOutput for SecretCreateView {
    const KIND: &'static str = "secret.create";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(transparent)]
pub struct SecretGetView(pub SecretView);

impl Masked for SecretGetView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for SecretGetView {
    fn message(&self) -> String {
        format!("Secret {}", format_message_highlight(&self.0.id),)
    }

    fn fields(&self) -> Vec<(String, String)> {
        secret_view_fields(&self.0)
    }
}

impl StructuredOutput for SecretGetView {
    const KIND: &'static str = "secret.get";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(transparent)]
pub struct SecretUpdateView(pub SecretView);

impl Masked for SecretUpdateView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for SecretUpdateView {
    fn message(&self) -> String {
        format!("Updated secret {}", format_message_highlight(&self.0.id),)
    }

    fn fields(&self) -> Vec<(String, String)> {
        secret_view_fields(&self.0)
    }
}

impl StructuredOutput for SecretUpdateView {
    const KIND: &'static str = "secret.update";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretDeleteView {
    pub deleted: bool,
    pub id: golem_common::model::agent_secret::AgentSecretId,
    pub environment_id: golem_common::model::environment::EnvironmentId,
    pub path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
    pub revision: golem_common::model::agent_secret::AgentSecretRevision,
}

impl From<SecretView> for SecretDeleteView {
    fn from(view: SecretView) -> Self {
        Self {
            deleted: true,
            id: view.id,
            environment_id: view.environment_id,
            path: view.path,
            revision: view.revision,
        }
    }
}

impl Masked for SecretDeleteView {}

impl MessageWithFields for SecretDeleteView {
    fn message(&self) -> String {
        format!("Deleted secret {}", format_message_highlight(&self.id))
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        fields
            .fmt_field("Environment ID", &self.environment_id.0, format_main_id)
            .fmt_field("Path", &self.path, format_main_id)
            .fmt_field("ID", &self.id, format_id)
            .fmt_field("Revision", &self.revision.get(), format_id);
        fields.build()
    }
}

impl StructuredOutput for SecretDeleteView {
    const KIND: &'static str = "secret.delete";
}

fn secret_view_fields(view: &SecretView) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Environment ID", &view.environment_id.0, format_main_id)
        .fmt_field("Path", &view.path, format_main_id)
        .fmt_field("ID", &view.id, format_id)
        .fmt_field("Revision", &view.revision.get(), format_id)
        .fmt_field("Secret Type", &view.secret_type, |graph| {
            render_type_for_language(&SourceLanguage::default(), graph, &graph.root, false)
        })
        .fmt_field("Has Value", &view.has_value, |value| value.to_string())
        .fmt_field("Destinations", &view.destinations, |values| {
            values.join(", ")
        });

    fields.build()
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
#[serde(rename_all = "camelCase")]
pub struct SecretListView {
    pub secrets: Vec<SecretView>,
    #[serde(skip)]
    pub environment_name: String,
    #[serde(skip)]
    pub show_ids: bool,
}

impl Masked for SecretListView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        self.secrets = self
            .secrets
            .into_iter()
            .map(|secret| secret.masked(config))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(self)
    }
}

impl SecretListView {
    fn table(&self) -> ComfyTable {
        let mut table = new_table_full_condensed(vec![
            Column::new("Environment").fixed(),
            Column::new("Path"),
            Column::new("Revision").fixed_right(),
            Column::new("Has Value"),
            Column::new("Destinations"),
        ]);

        if self.show_ids {
            table.set_header(vec![
                Cell::new("Environment"),
                Cell::new("Path"),
                Cell::new("Revision"),
                Cell::new("Has Value"),
                Cell::new("Destinations"),
                Cell::new("Environment ID"),
                Cell::new("Secret ID"),
            ]);
        }

        for secret in &self.secrets {
            if self.show_ids {
                table.add_row(vec![
                    self.environment_name.clone(),
                    secret.path.to_string(),
                    secret.revision.get().to_string(),
                    secret.has_value.to_string(),
                    secret.destinations.join(", "),
                    wrap_uuid_for_table(&secret.environment_id.0.to_string()),
                    wrap_uuid_for_table(&secret.id.to_string()),
                ]);
            } else {
                table.add_row(vec![
                    self.environment_name.clone(),
                    secret.path.to_string(),
                    secret.revision.get().to_string(),
                    secret.has_value.to_string(),
                    secret.destinations.join(", "),
                ]);
            }
        }

        table
    }
}

impl TextOutput for SecretListView {
    fn log(&self) {
        let table = self.table();
        log_table(table);
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        self.masked(config)?.log();
        Ok(())
    }
}

impl StructuredOutput for SecretListView {
    const KIND: &'static str = "secret.list";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretGetView, SecretListView, wrap_uuid_for_table};
    use crate::model::masking::{Masked, MaskingConfig};
    use golem_client::model::AgentSecretDto;
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::schema::{SchemaGraph, SchemaType};
    use serde_json::json;
    use test_r::test;

    fn sample_secret() -> super::SecretView {
        AgentSecretDto {
            id: "00000000-0000-0000-0000-000000000002"
                .parse::<AgentSecretId>()
                .unwrap(),
            environment_id: "00000000-0000-0000-0000-000000000001"
                .parse::<EnvironmentId>()
                .unwrap(),
            path: CanonicalAgentSecretPath(vec!["token".to_string()]),
            revision: AgentSecretRevision::new(7).unwrap(),
            secret_type: SchemaGraph::anonymous(SchemaType::string()),
            has_value: true,
            destinations: vec!["https://example.com".to_string()],
        }
        .into()
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
        let view = SecretListView {
            secrets: vec![sample_secret()],
            environment_name: "local".to_string(),
            show_ids: true,
        };

        let masked = view.masked(MaskingConfig::show_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert!(json.get("secrets").is_some());
        assert!(json.get("environment_name").is_none());
        assert!(json.get("show_ids").is_none());
    }

    #[test]
    fn list_view_json_exposes_secret_metadata() {
        let view = SecretListView {
            secrets: vec![sample_secret()],
            environment_name: "local".to_string(),
            show_ids: true,
        };

        let masked = view.masked(MaskingConfig::hide_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert_eq!(json["secrets"][0]["hasValue"], json!(true));
        assert_eq!(
            json["secrets"][0]["destinations"],
            json!(["https://example.com"])
        );
        assert!(json["secrets"][0].get("secretValue").is_none());
    }

    #[test]
    fn get_view_json_exposes_secret_metadata() {
        let view = SecretGetView(sample_secret());

        let masked = view.masked(MaskingConfig::hide_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert_eq!(json["hasValue"], json!(true));
        assert_eq!(json["destinations"], json!(["https://example.com"]));
        assert!(json.get("secretValue").is_none());
    }

    #[test]
    fn get_view_json_show_secrets_still_only_exposes_metadata() {
        let view = SecretGetView(sample_secret());

        let masked = view.masked(MaskingConfig::show_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert_eq!(json["hasValue"], json!(true));
        assert_eq!(json["destinations"], json!(["https://example.com"]));
        assert!(json.get("secretValue").is_none());
    }

    #[test]
    fn default_table_omits_ids_and_type_columns() {
        let view = SecretListView {
            secrets: vec![sample_secret()],
            environment_name: "local".to_string(),
            show_ids: false,
        };

        let masked = view.masked(MaskingConfig::hide_secrets()).unwrap();
        let rendered = masked.table().to_string();

        assert!(rendered.contains("Environment"));
        assert!(rendered.contains("Path"));
        assert!(rendered.contains("Revision"));
        assert!(rendered.contains("Has Value"));
        assert!(rendered.contains("Destinations"));
        assert!(!rendered.contains("Environment ID"));
        assert!(!rendered.contains("Secret ID"));
        assert!(!rendered.contains("Secret Type"));
        assert!(rendered.contains("local"));
        assert!(rendered.contains("token"));
        assert!(rendered.contains("true"));
        assert!(rendered.contains("https://example.com"));
    }

    #[test]
    fn ids_table_includes_wrapped_id_columns() {
        let view = SecretListView {
            secrets: vec![sample_secret()],
            environment_name: "local".to_string(),
            show_ids: true,
        };

        let masked = view.masked(MaskingConfig::hide_secrets()).unwrap();
        let rendered = masked.table().to_string();

        assert!(rendered.contains("local"));
        assert!(rendered.contains("token"));
        assert!(!rendered.contains("Secret Type"));
        assert!(!rendered.is_empty());
    }
}
