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
use crate::model::masking::{Masked, MaskingConfig, mask_json_secret_value};
use crate::model::text::fmt::*;

use comfy_table::Cell;
use golem_common::model::agent_secret::AgentSecretDto;
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
    pub secret_type: golem_wasm::analysis::AnalysedType,
    pub secret_value: Option<serde_json::Value>,
}

impl From<AgentSecretDto> for SecretView {
    fn from(value: AgentSecretDto) -> Self {
        Self {
            id: value.id,
            environment_id: value.environment_id,
            path: value.path,
            revision: value.revision,
            secret_type: value.secret_type,
            secret_value: value.secret_value,
        }
    }
}

impl Masked for SecretView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        self.secret_value = mask_json_secret_value(config, &self.secret_value);
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(transparent)]
pub struct SecretCreateView(pub SecretView);

impl Masked for SecretCreateView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for SecretCreateView {
    fn message(&self) -> String {
        format!(
            "Created a new secret {}",
            format_message_highlight(&self.0.id),
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        secret_view_fields(&self.0)
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
    const KIND: &'static str = "secret.update-value";

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
pub struct SecretDeleteView(pub SecretView);

impl Masked for SecretDeleteView {
    fn masked(self, config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(Self(self.0.masked(config)?))
    }
}

impl MessageWithFields for SecretDeleteView {
    fn message(&self) -> String {
        format!("Deleted secret {}", format_message_highlight(&self.0.id),)
    }

    fn fields(&self) -> Vec<(String, String)> {
        secret_view_fields(&self.0)
    }
}

impl StructuredOutput for SecretDeleteView {
    const KIND: &'static str = "secret.delete";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

fn secret_view_fields(view: &SecretView) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Environment ID", &view.environment_id.0, format_main_id)
        .fmt_field("Path", &view.path, format_main_id)
        .fmt_field("ID", &view.id, format_id)
        .fmt_field("Revision", &view.revision.get(), format_id)
        .fmt_field("Secret Type", &view.secret_type, |st| {
            // Adapt the legacy AnalysedType at the boundary into a schema graph
            // before delegating to the schema-typed type renderer.
            match golem_common::schema::adapters::analysed_type_to_schema_graph(st) {
                Ok(graph) => {
                    let root = graph.root.clone();
                    render_type_for_language(&SourceLanguage::default(), &graph, &root, false)
                }
                Err(_) => "<unknown>".to_string(),
            }
        })
        .fmt_field_option("Secret Value", &view.secret_value, |v| v.to_string());

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
            let secret_value = secret
                .secret_value
                .as_ref()
                .unwrap_or(&serde_json::Value::Null)
                .to_string();

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
    use golem_common::model::agent_secret::{
        AgentSecretDto, AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_wasm::analysis::analysed_type::str;
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
            secret_type: str(),
            secret_value: Some(json!("super-secret")),
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
    fn list_view_json_masks_secret_values_by_default() {
        let view = SecretListView {
            secrets: vec![sample_secret()],
            environment_name: "local".to_string(),
            show_ids: true,
        };

        let masked = view.masked(MaskingConfig::hide_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert_eq!(json["secrets"][0]["secretValue"], json!("***"));
        assert!(!json.to_string().contains("super-secret"));
    }

    #[test]
    fn get_view_json_masks_secret_value_by_default() {
        let view = SecretGetView(sample_secret());

        let masked = view.masked(MaskingConfig::hide_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert_eq!(json["secretValue"], json!("***"));
        assert!(!json.to_string().contains("super-secret"));
    }

    #[test]
    fn get_view_json_shows_secret_value_when_requested() {
        let view = SecretGetView(sample_secret());

        let masked = view.masked(MaskingConfig::show_secrets()).unwrap();
        let json = serde_json::to_value(&masked).unwrap();

        assert_eq!(json["secretValue"], json!("super-secret"));
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
