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

use crate::agent_id_display::{SourceLanguage, parse_type_for_language, parse_value_for_language};
use crate::command::api::secret::SecretSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::AnyhowMapServiceError;
use crate::log::log_error;
use crate::model::GuestLanguage;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::text::secret::{
    SecretCreateView, SecretDeleteView, SecretGetView, SecretListView, SecretUpdateView,
};
use anyhow::bail;
use golem_client::api::AgentSecretsClient;
use golem_client::model::AgentSecretUpdate;
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretDto, AgentSecretId, AgentSecretPath, CanonicalAgentSecretPath,
};
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct SecretCommandHandler {
    ctx: Arc<Context>,
}

impl SecretCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: SecretSubcommand) -> anyhow::Result<()> {
        match command {
            SecretSubcommand::Create {
                path,
                secret_type,
                secret_value,
            } => self.cmd_create(path, secret_type, secret_value).await,
            SecretSubcommand::Get { path, id } => self.cmd_get(path, id).await,
            SecretSubcommand::UpdateValue {
                path,
                id,
                secret_value,
            } => self.cmd_update_value(path, id, secret_value).await,
            SecretSubcommand::Delete { path, id } => self.cmd_delete(path, id).await,
            SecretSubcommand::List { ids } => self.cmd_list(ids).await,
        }
    }

    async fn resolve_secret(
        &self,
        path: Option<AgentSecretPath>,
        id: Option<AgentSecretId>,
    ) -> anyhow::Result<AgentSecretDto> {
        let clients = self.ctx.golem_clients().await?;

        if let Some(path) = path {
            let environment = self
                .ctx
                .environment_handler()
                .resolve_environment(EnvironmentResolveMode::Any)
                .await?;

            let canonical = CanonicalAgentSecretPath::from_path_in_unknown_casing(&path.0);

            let secrets = clients
                .agent_secrets
                .list_environment_agent_secrets(&environment.environment_id.0)
                .await
                .map_service_error()?
                .values;

            match secrets.into_iter().find(|s| s.path == canonical) {
                Some(secret) => Ok(secret),
                None => {
                    log_error(format!(
                        "Agent secret with path '{}' not found in environment",
                        canonical
                    ));
                    bail!(NonSuccessfulExit);
                }
            }
        } else if let Some(id) = id {
            Ok(clients
                .agent_secrets
                .get_agent_secret(&id.0)
                .await
                .map_service_error()?)
        } else {
            log_error("Either path or id must be provided");
            bail!(NonSuccessfulExit);
        }
    }

    async fn cmd_create(
        &self,
        path: AgentSecretPath,
        secret_type: String,
        secret_value: Option<String>,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let source_language = self.guess_source_language().await;
        let secret_type: AnalysedType =
            match parse_type_for_language(&secret_type, &source_language) {
                Ok(res) => res,
                Err(_) => {
                    // If the detected language parser fails, try all other parsers
                    match parse_type_for_language(
                        &secret_type,
                        &SourceLanguage::Other(String::new()),
                    ) {
                        Ok(res) => res,
                        Err(_) => match serde_json::from_str(&secret_type) {
                            Ok(res) => res,
                            Err(json_err) => {
                                log_error(format!("Malformed secret type provided: {json_err}"));
                                bail!(NonSuccessfulExit);
                            }
                        },
                    }
                }
            };

        let secret_value: Option<serde_json::Value> = match secret_value {
            Some(sv) => Some(self.parse_secret_value(&sv, &secret_type, &source_language)?),
            None => None,
        };

        let result = clients
            .agent_secrets
            .create_agent_secret(
                &environment.environment_id.0,
                &AgentSecretCreation {
                    path,
                    secret_type,
                    secret_value,
                },
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&SecretCreateView {
            secret: result,
            show_sensitive: self.ctx.show_sensitive(),
        });

        Ok(())
    }

    async fn cmd_get(
        &self,
        path: Option<AgentSecretPath>,
        id: Option<AgentSecretId>,
    ) -> anyhow::Result<()> {
        let result = self.resolve_secret(path, id).await?;

        self.ctx.log_handler().log_view(&SecretGetView {
            secret: result,
            show_sensitive: self.ctx.show_sensitive(),
        });

        Ok(())
    }

    async fn cmd_update_value(
        &self,
        path: Option<AgentSecretPath>,
        id: Option<AgentSecretId>,
        secret_value: Option<String>,
    ) -> anyhow::Result<()> {
        let current = self.resolve_secret(path, id).await?;
        let source_language = self.guess_source_language().await;

        let clients = self.ctx.golem_clients().await?;

        let secret_value: Option<serde_json::Value> = match secret_value {
            Some(sv) => {
                Some(self.parse_secret_value(&sv, &current.secret_type, &source_language)?)
            }
            None => None,
        };

        let result = clients
            .agent_secrets
            .update_agent_secret(
                &current.id.0,
                &AgentSecretUpdate {
                    current_revision: current.revision,
                    secret_value: OptionalFieldUpdate::update_from_option(secret_value),
                },
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&SecretUpdateView {
            secret: result,
            show_sensitive: self.ctx.show_sensitive(),
        });

        Ok(())
    }

    async fn cmd_delete(
        &self,
        path: Option<AgentSecretPath>,
        id: Option<AgentSecretId>,
    ) -> anyhow::Result<()> {
        let current = self.resolve_secret(path, id).await?;

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .agent_secrets
            .delete_agent_secret(&current.id.0, current.revision.into())
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&SecretDeleteView {
            secret: result,
            show_sensitive: self.ctx.show_sensitive(),
        });

        Ok(())
    }

    async fn guess_source_language(&self) -> SourceLanguage {
        let app_state = self.ctx.app_context_lock().await;
        if let Ok(Some(app_ctx)) = app_state.opt() {
            let app = app_ctx.application();
            let mut languages: BTreeSet<GuestLanguage> = BTreeSet::new();
            for name in app.component_names() {
                if let Some(lang) = app.component(name).guess_language() {
                    languages.insert(lang);
                }
            }
            if languages.len() == 1 {
                let lang = languages.into_iter().next().unwrap();
                return match lang {
                    GuestLanguage::Rust => SourceLanguage::Rust,
                    GuestLanguage::TypeScript => SourceLanguage::TypeScript,
                    GuestLanguage::Scala => SourceLanguage::Scala,
                    GuestLanguage::MoonBit => SourceLanguage::MoonBit,
                };
            }
        }
        SourceLanguage::Other(String::new())
    }

    fn parse_secret_value(
        &self,
        input: &str,
        secret_type: &AnalysedType,
        source_language: &SourceLanguage,
    ) -> anyhow::Result<serde_json::Value> {
        match parse_secret_value_to_json(input, secret_type, source_language) {
            Ok(json) => Ok(json),
            Err(msg) => {
                log_error(msg);
                bail!(NonSuccessfulExit);
            }
        }
    }

    async fn cmd_list(&self, show_ids: bool) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let results = clients
            .agent_secrets
            .list_environment_agent_secrets(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&SecretListView {
            show_sensitive: self.ctx.show_sensitive(),
            environment_name: environment.environment_name.0,
            show_ids,
            secrets: results,
        });

        Ok(())
    }
}

fn parse_secret_value_to_json(
    input: &str,
    secret_type: &AnalysedType,
    source_language: &SourceLanguage,
) -> Result<serde_json::Value, String> {
    // Try language-specific parser first
    if let Ok(vat) = parse_value_for_language(input, secret_type, source_language)
        && let Ok(json) = vat.to_json_value()
    {
        return Ok(json);
    }
    // Fall back to raw JSON
    match serde_json::from_str(input) {
        Ok(json) => Ok(json),
        Err(err) => {
            // If the expected type is a plain string and the input is not valid JSON,
            // treat the raw input as the string value (ergonomic fallback).
            if matches!(secret_type, AnalysedType::Str(_)) {
                Ok(serde_json::Value::String(input.to_string()))
            } else {
                Err(format!("Secret value is not valid: {err}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::command::{GolemCliCommand, GolemCliSubcommand};
    use clap::Parser;
    use golem_wasm::analysis::{
        NameTypePair, TypeBool, TypeF64, TypeList, TypeOption, TypeRecord, TypeS32, TypeStr, TypeU32,
    };

    fn str_type() -> AnalysedType {
        AnalysedType::Str(TypeStr)
    }

    fn u32_type() -> AnalysedType {
        AnalysedType::U32(TypeU32)
    }

    fn option_u32_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            name: None,
            owner: None,
            inner: Box::new(AnalysedType::U32(TypeU32)),
        })
    }

    fn list_u32_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            name: None,
            owner: None,
            inner: Box::new(AnalysedType::U32(TypeU32)),
        })
    }

    fn lang() -> SourceLanguage {
        SourceLanguage::Other(String::new())
    }

    // --- String type ---

    #[test]
    fn bare_string_accepted_for_str_type() {
        let result = parse_secret_value_to_json("sk-abc123", &str_type(), &lang());
        assert_eq!(result, Ok(serde_json::Value::String("sk-abc123".to_string())));
    }

    #[test]
    fn json_quoted_string_accepted_for_str_type() {
        let result = parse_secret_value_to_json(r#""sk-abc123""#, &str_type(), &lang());
        assert_eq!(result, Ok(serde_json::Value::String("sk-abc123".to_string())));
    }

    #[test]
    fn bare_string_with_spaces_accepted_for_str_type() {
        let result = parse_secret_value_to_json("hello world", &str_type(), &lang());
        assert_eq!(result, Ok(serde_json::Value::String("hello world".to_string())));
    }

    #[test]
    fn bare_string_api_key_like_accepted_for_str_type() {
        let result = parse_secret_value_to_json("sk-abc123.endpoint/v2", &str_type(), &lang());
        assert_eq!(
            result,
            Ok(serde_json::Value::String("sk-abc123.endpoint/v2".to_string()))
        );
    }

    // --- Numeric types ---

    #[test]
    fn valid_json_number_accepted_for_u32_type() {
        let result = parse_secret_value_to_json("42", &u32_type(), &lang());
        assert_eq!(result, Ok(serde_json::json!(42)));
    }

    #[test]
    fn negative_number_accepted_for_s32_type() {
        let result = parse_secret_value_to_json("-7", &AnalysedType::S32(TypeS32), &lang());
        assert_eq!(result, Ok(serde_json::json!(-7)));
    }

    #[test]
    fn decimal_accepted_for_f64_type() {
        let result = parse_secret_value_to_json("3.14", &AnalysedType::F64(TypeF64), &lang());
        assert_eq!(result, Ok(serde_json::json!(3.14)));
    }

    // --- Bool type ---

    #[test]
    fn bool_true_accepted() {
        let result = parse_secret_value_to_json("true", &AnalysedType::Bool(TypeBool), &lang());
        assert_eq!(result, Ok(serde_json::json!(true)));
    }

    #[test]
    fn bool_false_accepted() {
        let result = parse_secret_value_to_json("false", &AnalysedType::Bool(TypeBool), &lang());
        assert_eq!(result, Ok(serde_json::json!(false)));
    }

    // --- Option type ---

    #[test]
    fn option_null_accepted() {
        let result = parse_secret_value_to_json("null", &option_u32_type(), &lang());
        assert_eq!(result, Ok(serde_json::json!(null)));
    }

    #[test]
    fn option_inner_value_accepted() {
        let result = parse_secret_value_to_json("42", &option_u32_type(), &lang());
        assert_eq!(result, Ok(serde_json::json!(42)));
    }

    // --- List type ---

    #[test]
    fn list_json_array_accepted() {
        let result = parse_secret_value_to_json("[1,2,3]", &list_u32_type(), &lang());
        assert_eq!(result, Ok(serde_json::json!([1, 2, 3])));
    }

    // --- Record type ---

    #[test]
    fn record_json_object_accepted() {
        let rec_type = AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
            fields: vec![
                NameTypePair {
                    name: "host".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "port".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        });
        let result =
            parse_secret_value_to_json(r#"{"host":"localhost","port":5432}"#, &rec_type, &lang());
        assert_eq!(
            result,
            Ok(serde_json::json!({"host": "localhost", "port": 5432}))
        );
    }

    // --- Language-specific: Rust dialect ---

    #[test]
    fn rust_dialect_some_option_accepted() {
        let result =
            parse_secret_value_to_json("Some(42)", &option_u32_type(), &SourceLanguage::Rust);
        assert_eq!(result, Ok(serde_json::json!(42)));
    }

    #[test]
    fn rust_dialect_none_option_accepted() {
        let result =
            parse_secret_value_to_json("None", &option_u32_type(), &SourceLanguage::Rust);
        assert_eq!(result, Ok(serde_json::json!(null)));
    }

    // --- Error cases ---

    #[test]
    fn bare_word_rejected_for_non_string_type() {
        let result = parse_secret_value_to_json("notANumber", &u32_type(), &lang());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Secret value is not valid"));
    }

    // --- CLI argument ordering ---

    fn extract_create_args(cmd: GolemCliCommand) -> (String, Option<String>) {
        match cmd.subcommand {
            GolemCliSubcommand::AgentSecret {
                subcommand:
                    AgentSecretSubcommand::Create {
                        secret_type,
                        secret_value,
                        ..
                    },
            } => (secret_type, secret_value),
            _ => panic!("expected AgentSecret Create"),
        }
    }

    #[test]
    fn create_type_before_value_parses() {
        let cmd = GolemCliCommand::try_parse_from([
            "golem-cli",
            "agent-secret",
            "create",
            "apiKey",
            "--secret-type",
            "String",
            "--secret-value",
            "sk-abc123",
        ])
        .expect("parse failed");
        let (typ, val) = extract_create_args(cmd);
        assert_eq!(typ, "String");
        assert_eq!(val.as_deref(), Some("sk-abc123"));
    }

    #[test]
    fn create_value_before_type_parses() {
        let cmd = GolemCliCommand::try_parse_from([
            "golem-cli",
            "agent-secret",
            "create",
            "apiKey",
            "--secret-value",
            "sk-abc123",
            "--secret-type",
            "String",
        ])
        .expect("parse failed");
        let (typ, val) = extract_create_args(cmd);
        assert_eq!(typ, "String");
        assert_eq!(val.as_deref(), Some("sk-abc123"));
    }

    #[test]
    fn create_both_orderings_produce_same_args() {
        let order1 = GolemCliCommand::try_parse_from([
            "golem-cli",
            "agent-secret",
            "create",
            "apiKey",
            "--secret-type",
            "String",
            "--secret-value",
            "sk-abc123",
        ])
        .unwrap();
        let order2 = GolemCliCommand::try_parse_from([
            "golem-cli",
            "agent-secret",
            "create",
            "apiKey",
            "--secret-value",
            "sk-abc123",
            "--secret-type",
            "String",
        ])
        .unwrap();
        assert_eq!(extract_create_args(order1), extract_create_args(order2));
    }
}
