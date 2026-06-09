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
use crate::error::service::MapServiceError;
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
        // The new schema-typed parser returns `(SchemaGraph, SchemaType)`; the
        // legacy REST DTO still carries an `AnalysedType` for the secret type,
        // so adapt at the boundary here.
        let secret_type: AnalysedType =
            match parse_type_for_language(&secret_type, &source_language) {
                Ok((graph, ty)) => schema_to_analysed_type_at_boundary(&graph, &ty)?,
                Err(_) => {
                    // If the detected language parser fails, try all other parsers
                    match parse_type_for_language(
                        &secret_type,
                        &SourceLanguage::Other(String::new()),
                    ) {
                        Ok((graph, ty)) => schema_to_analysed_type_at_boundary(&graph, &ty)?,
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
    // Try the schema-typed language-specific parser first, then fall back to
    // raw JSON. We convert at the boundary because the REST DTO still
    // carries the legacy `AnalysedType` form.
    let graph = golem_common::schema::adapters::analysed_type_to_schema_graph(secret_type)
        .map_err(|err| format!("schema adapter error while parsing secret value: {err}"))?;
    if let Ok(parsed_value) = parse_value_for_language(input, &graph, &graph.root, source_language)
        && let Ok(legacy_value) = golem_common::schema::adapters::schema_value_to_value(
            &graph,
            &graph.root,
            &parsed_value,
        )
        && let Ok(json) =
            golem_wasm::ValueAndType::new(legacy_value, secret_type.clone()).to_json_value()
    {
        return Ok(json);
    }
    // Fall back to raw JSON, but only accept it if it coerces into
    // `secret_type`. Without this validation a user could store any JSON
    // for any secret type and the mismatch would only surface much later
    // at the consumer.
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(json) => {
            golem_wasm::ValueAndType::parse_with_type(&json, secret_type).map_err(|errs| {
                format!(
                    "Secret value does not match the expected type: {}",
                    errs.join("; ")
                )
            })?;
            Ok(json)
        }
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

/// Boundary helper: convert a parsed `(SchemaGraph, SchemaType)` back to
/// the legacy `AnalysedType` for the REST DTO. Logs and bails on failure.
fn schema_to_analysed_type_at_boundary(
    graph: &golem_common::schema::graph::SchemaGraph,
    ty: &golem_common::schema::schema_type::SchemaType,
) -> anyhow::Result<AnalysedType> {
    match golem_common::schema::adapters::schema_type_to_analysed_type(graph, ty) {
        Ok(ty) => Ok(ty),
        Err(err) => {
            log_error(format!(
                "Unsupported secret type (cannot map to legacy AnalysedType): {err}"
            ));
            bail!(NonSuccessfulExit);
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use golem_wasm::analysis::{
        NameTypePair, TypeBool, TypeF64, TypeList, TypeOption, TypeRecord, TypeS32, TypeStr,
        TypeU32,
    };
    use proptest::prelude::*;

    fn other_lang() -> SourceLanguage {
        SourceLanguage::Other(String::new())
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

    /// Generates an arbitrary record schema paired with a matching JSON object,
    /// with 1–5 fields of mixed simple types (String, U32, S32, F64, Bool).
    /// Uses hash_map to guarantee unique field names.
    fn arb_record_schema_and_json() -> impl Strategy<Value = (AnalysedType, serde_json::Value)> {
        let arb_field_value = prop_oneof![
            any::<String>()
                .prop_map(|s| (AnalysedType::Str(TypeStr), serde_json::Value::String(s))),
            any::<u32>().prop_map(|n| (AnalysedType::U32(TypeU32), serde_json::json!(n))),
            any::<i32>().prop_map(|n| (AnalysedType::S32(TypeS32), serde_json::json!(n))),
            decimal_f64_input().prop_map(|s| {
                let v = serde_json::from_str::<serde_json::Value>(&s).unwrap();
                (AnalysedType::F64(TypeF64), v)
            }),
            any::<bool>().prop_map(|b| (AnalysedType::Bool(TypeBool), serde_json::json!(b))),
        ];
        proptest::collection::hash_map("[a-z][a-zA-Z0-9]{0,8}", arb_field_value, 1..=5).prop_map(
            |fields| {
                let mut json_map = serde_json::Map::new();
                let type_fields: Vec<NameTypePair> = fields
                    .into_iter()
                    .map(|(name, (typ, val))| {
                        json_map.insert(name.clone(), val);
                        NameTypePair { name, typ }
                    })
                    .collect();
                (
                    AnalysedType::Record(TypeRecord {
                        name: None,
                        owner: None,
                        fields: type_fields,
                    }),
                    serde_json::Value::Object(json_map),
                )
            },
        )
    }

    /// Identifiers, API keys, paths — strings whose first character cannot
    /// start any JSON value (keywords t/f/n, digits, `-`, `"`, `[`, `{`),
    /// so they are structurally guaranteed to not parse as JSON.
    /// Covers:
    /// 1. API keys: sk-abc123, Bearer token123
    /// 2. Connection strings: postgres://user:pass@localhost/db
    /// 3. Bare identifiers: mySecretKey
    /// 4. Paths: path/to/neverland
    fn bare_secret_value() -> impl Strategy<Value = String> {
        "[a-eg-mo-su-zA-Z][a-zA-Z0-9_./@: -]*"
    }

    /// Generates valid JSON string literals by serializing arbitrary Rust strings
    /// through serde_json, guaranteeing the output is always a properly quoted
    /// and escaped JSON value (e.g. `"hello"`, `"line\nbreak"`, `"has \"quotes\""`)
    fn json_encoded_string() -> impl Strategy<Value = String> {
        any::<String>().prop_map(|s| serde_json::to_string(&s).unwrap())
    }

    /// Decimal numbers as a user would type them as a secret value: `3.14`, `-7.0`, `0.9999`.
    fn decimal_f64_input() -> impl Strategy<Value = String> {
        (any::<i32>(), 0u32..=9999u32)
            .prop_map(|(int_part, frac_part)| format!("{int_part}.{frac_part}"))
    }

    proptest! {
        #[test]
        fn non_json_str_returned_as_is(input in bare_secret_value()) {
            let result = parse_secret_value_to_json(&input, &AnalysedType::Str(TypeStr), &other_lang());
            prop_assert_eq!(result, Ok(serde_json::Value::String(input)));
        }

        #[test]
        fn json_encoded_str_is_decoded(json_input in json_encoded_string()) {
            let expected: String = serde_json::from_str(&json_input).unwrap();
            let result = parse_secret_value_to_json(&json_input, &AnalysedType::Str(TypeStr), &other_lang());
            prop_assert_eq!(result, Ok(serde_json::Value::String(expected)));
        }

        #[test]
        fn decimal_string_accepted_for_u32_type(n in any::<u32>()) {
            let result = parse_secret_value_to_json(&n.to_string(), &AnalysedType::U32(TypeU32), &other_lang());
            prop_assert_eq!(result, Ok(serde_json::json!(n)));
        }

        #[test]
        fn decimal_string_accepted_for_s32_type(n in any::<i32>()) {
            let result = parse_secret_value_to_json(&n.to_string(), &AnalysedType::S32(TypeS32), &other_lang());
            prop_assert_eq!(result, Ok(serde_json::json!(n)));
        }

        #[test]
        fn decimal_string_accepted_for_f64_type(input in decimal_f64_input()) {
            let result = parse_secret_value_to_json(&input, &AnalysedType::F64(TypeF64), &other_lang());
            let expected = serde_json::from_str::<serde_json::Value>(&input).unwrap();
            prop_assert_eq!(result, Ok(expected));
        }

        #[test]
        fn bool_literal_accepted_for_bool_type(b in any::<bool>()) {
            let result = parse_secret_value_to_json(&b.to_string(), &AnalysedType::Bool(TypeBool), &other_lang());
            prop_assert_eq!(result, Ok(serde_json::json!(b)));
        }

        #[test]
        fn json_array_accepted_for_list_type(values in proptest::collection::vec(any::<u32>(), 0..=20)) {
            let json_str = serde_json::to_string(&values).unwrap();
            let result = parse_secret_value_to_json(&json_str, &list_u32_type(), &other_lang());
            let expected: serde_json::Value = serde_json::to_value(&values).unwrap();
            prop_assert_eq!(result, Ok(expected));
        }

        #[test]
        fn json_object_accepted_for_record_type((rec_type, json) in arb_record_schema_and_json()) {
            let result = parse_secret_value_to_json(&json.to_string(), &rec_type, &other_lang());
            prop_assert_eq!(result, Ok(json));
        }

        #[test]
        fn non_numeric_input_rejected_for_numeric_type(
            input in bare_secret_value(),
            typ in prop_oneof![
                Just(AnalysedType::U32(TypeU32)),
                Just(AnalysedType::S32(TypeS32)),
                Just(AnalysedType::F64(TypeF64)),
            ],
        ) {
            let result = parse_secret_value_to_json(&input, &typ, &other_lang());
            prop_assert!(result.is_err());
            prop_assert!(result.unwrap_err().contains("Secret value is not valid"));
        }
    }

    #[test]
    fn option_null_accepted() {
        assert_eq!(
            parse_secret_value_to_json("null", &option_u32_type(), &other_lang()),
            Ok(serde_json::json!(null))
        );
    }
}
