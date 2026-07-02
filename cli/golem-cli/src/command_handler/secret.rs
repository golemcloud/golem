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
use golem_common::schema::validation::validate_value;
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
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
        // The schema-typed parser returns `(SchemaGraph, SchemaType)` where the
        // graph's root is the parsed type; the REST DTO carries the secret type
        // as a schema-native `SchemaGraph` directly.
        let secret_type: SchemaGraph = match parse_type_for_language(&secret_type, &source_language)
        {
            Ok((graph, _ty)) => graph,
            Err(_) => {
                // If the detected language parser fails, try all other parsers
                match parse_type_for_language(&secret_type, &SourceLanguage::Other(String::new())) {
                    Ok((graph, _ty)) => graph,
                    Err(_) => match serde_json::from_str::<SchemaGraph>(&secret_type) {
                        Ok(graph) => graph,
                        Err(json_err) => {
                            log_error(format!("Malformed secret type provided: {json_err}"));
                            bail!(NonSuccessfulExit);
                        }
                    },
                }
            }
        };

        let secret_value: Option<SchemaValue> = match secret_value {
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

        self.ctx
            .log_handler()
            .log_output(SecretCreateView(result.into()))?;

        Ok(())
    }

    async fn cmd_get(
        &self,
        path: Option<AgentSecretPath>,
        id: Option<AgentSecretId>,
    ) -> anyhow::Result<()> {
        let result = self.resolve_secret(path, id).await?;

        self.ctx
            .log_handler()
            .log_output(SecretGetView(result.into()))?;

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

        let secret_value: Option<SchemaValue> = match secret_value {
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

        self.ctx
            .log_handler()
            .log_output(SecretUpdateView(result.into()))?;

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

        self.ctx
            .log_handler()
            .log_output(SecretDeleteView(result.into()))?;

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
                    GuestLanguage::TypeScriptFluent => SourceLanguage::TypeScript,
                };
            }
        }
        SourceLanguage::Other(String::new())
    }

    fn parse_secret_value(
        &self,
        input: &str,
        secret_type: &SchemaGraph,
        source_language: &SourceLanguage,
    ) -> anyhow::Result<SchemaValue> {
        match parse_secret_value_to_schema_value(input, secret_type, source_language) {
            Ok(value) => Ok(value),
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

        self.ctx.log_handler().log_output(SecretListView {
            environment_name: environment.environment_name.0,
            show_ids,
            secrets: results.into_iter().map(Into::into).collect(),
        })?;

        Ok(())
    }
}

fn parse_secret_value_to_schema_value(
    input: &str,
    secret_type: &SchemaGraph,
    source_language: &SourceLanguage,
) -> Result<SchemaValue, String> {
    // Try the schema-typed language-specific value parser first. It accepts
    // ergonomic, language-flavored input (e.g. bare numbers, unquoted enum
    // cases) and produces a schema-native `SchemaValue` directly.
    if let Ok(parsed_value) =
        parse_value_for_language(input, secret_type, &secret_type.root, source_language)
    {
        return Ok(parsed_value);
    }
    // Fall back to a raw schema-native `SchemaValue` JSON, but only accept it
    // if it conforms to `secret_type`. Without this validation a user could
    // store any value for any secret type and the mismatch would only surface
    // much later at the consumer.
    match serde_json::from_str::<SchemaValue>(input) {
        Ok(value) => {
            validate_value(secret_type, &secret_type.root, &value).map_err(|errs| {
                format!(
                    "Secret value does not match the expected type: {}",
                    errs.iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            })?;
            Ok(value)
        }
        Err(err) => {
            // If the expected type is a plain string and the input is not valid
            // JSON, treat the raw input as the string value (ergonomic fallback).
            if matches!(secret_type.root, SchemaType::String { .. }) {
                Ok(SchemaValue::String(input.to_string()))
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
    use golem_common::schema::schema_type::NamedFieldType;
    use proptest::prelude::*;

    fn other_lang() -> SourceLanguage {
        SourceLanguage::Other(String::new())
    }

    fn graph_of(root: SchemaType) -> SchemaGraph {
        SchemaGraph::anonymous(root)
    }

    fn option_u32_graph() -> SchemaGraph {
        graph_of(SchemaType::option(SchemaType::u32()))
    }

    fn list_u32_graph() -> SchemaGraph {
        graph_of(SchemaType::list(SchemaType::u32()))
    }

    /// Generates an arbitrary record schema paired with a matching
    /// `SchemaValue`, with 1–5 fields of mixed simple types (String, U32, S32,
    /// F64, Bool). Field names are pure lowercase so the TS renderer/parser
    /// round-trips them identically (lowerCamelCase is the identity here) and
    /// `hash_map` guarantees uniqueness.
    fn arb_record_schema_and_value() -> impl Strategy<Value = (SchemaType, SchemaValue)> {
        let arb_field = prop_oneof![
            any::<String>().prop_map(|s| (SchemaType::string(), SchemaValue::String(s))),
            any::<u32>().prop_map(|n| (SchemaType::u32(), SchemaValue::U32(n))),
            any::<i32>().prop_map(|n| (SchemaType::s32(), SchemaValue::S32(n))),
            decimal_f64_input().prop_map(|s| {
                (
                    SchemaType::f64(),
                    SchemaValue::F64(s.parse::<f64>().unwrap()),
                )
            }),
            any::<bool>().prop_map(|b| (SchemaType::bool(), SchemaValue::Bool(b))),
        ];
        proptest::collection::hash_map("[a-z]{1,8}", arb_field, 1..=5).prop_map(|fields| {
            let mut type_fields = Vec::with_capacity(fields.len());
            let mut value_fields = Vec::with_capacity(fields.len());
            for (name, (typ, val)) in fields {
                type_fields.push(NamedFieldType {
                    name,
                    body: typ,
                    metadata: Default::default(),
                });
                value_fields.push(val);
            }
            (
                SchemaType::record(type_fields),
                SchemaValue::Record {
                    fields: value_fields,
                },
            )
        })
    }

    /// Identifiers, API keys, paths — strings whose first character cannot
    /// start any JSON value (keywords t/f/n, digits, `-`, `"`, `[`, `{`),
    /// so they are structurally guaranteed to not parse as a language value
    /// or a raw `SchemaValue` JSON, exercising the bare-string fallback.
    /// Covers:
    /// 1. API keys: sk-abc123, Bearer token123
    /// 2. Connection strings: postgres://user:pass@localhost/db
    /// 3. Bare identifiers: mySecretKey
    /// 4. Paths: path/to/neverland
    fn bare_secret_value() -> impl Strategy<Value = String> {
        "[a-eg-mo-su-zA-Z][a-zA-Z0-9_./@: -]*"
    }

    /// Generates valid double-quoted string literals by serializing arbitrary
    /// Rust strings through serde_json, guaranteeing the output is always a
    /// properly quoted and escaped value (e.g. `"hello"`, `"line\nbreak"`,
    /// `"has \"quotes\""`) that the language value parser accepts.
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
            let result = parse_secret_value_to_schema_value(&input, &graph_of(SchemaType::string()), &other_lang());
            prop_assert_eq!(result, Ok(SchemaValue::String(input)));
        }

        #[test]
        fn json_encoded_str_is_decoded(json_input in json_encoded_string()) {
            let expected: String = serde_json::from_str(&json_input).unwrap();
            let result = parse_secret_value_to_schema_value(&json_input, &graph_of(SchemaType::string()), &other_lang());
            prop_assert_eq!(result, Ok(SchemaValue::String(expected)));
        }

        #[test]
        fn decimal_string_accepted_for_u32_type(n in any::<u32>()) {
            let result = parse_secret_value_to_schema_value(&n.to_string(), &graph_of(SchemaType::u32()), &other_lang());
            prop_assert_eq!(result, Ok(SchemaValue::U32(n)));
        }

        #[test]
        fn decimal_string_accepted_for_s32_type(n in any::<i32>()) {
            let result = parse_secret_value_to_schema_value(&n.to_string(), &graph_of(SchemaType::s32()), &other_lang());
            prop_assert_eq!(result, Ok(SchemaValue::S32(n)));
        }

        #[test]
        fn decimal_string_accepted_for_f64_type(input in decimal_f64_input()) {
            let result = parse_secret_value_to_schema_value(&input, &graph_of(SchemaType::f64()), &other_lang());
            prop_assert_eq!(result, Ok(SchemaValue::F64(input.parse::<f64>().unwrap())));
        }

        #[test]
        fn bool_literal_accepted_for_bool_type(b in any::<bool>()) {
            let result = parse_secret_value_to_schema_value(&b.to_string(), &graph_of(SchemaType::bool()), &other_lang());
            prop_assert_eq!(result, Ok(SchemaValue::Bool(b)));
        }

        #[test]
        fn array_accepted_for_list_type(values in proptest::collection::vec(any::<u32>(), 0..=20)) {
            let input = format!("[{}]", values.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", "));
            let result = parse_secret_value_to_schema_value(&input, &list_u32_graph(), &other_lang());
            let expected = SchemaValue::List {
                elements: values.into_iter().map(SchemaValue::U32).collect(),
            };
            prop_assert_eq!(result, Ok(expected));
        }

        #[test]
        fn object_accepted_for_record_type((rec_type, value) in arb_record_schema_and_value()) {
            let graph = graph_of(rec_type.clone());
            let input = crate::agent_id_display::render_schema_value(&graph, &rec_type, &value, &other_lang());
            let result = parse_secret_value_to_schema_value(&input, &graph, &other_lang());
            prop_assert_eq!(result, Ok(value));
        }

        #[test]
        fn non_numeric_input_rejected_for_numeric_type(
            input in bare_secret_value(),
            typ in prop_oneof![
                Just(SchemaType::u32()),
                Just(SchemaType::s32()),
                Just(SchemaType::f64()),
            ],
        ) {
            let result = parse_secret_value_to_schema_value(&input, &graph_of(typ), &other_lang());
            prop_assert!(result.is_err());
            prop_assert!(result.unwrap_err().contains("Secret value is not valid"));
        }
    }

    #[test]
    fn option_null_accepted() {
        assert_eq!(
            parse_secret_value_to_schema_value("null", &option_u32_graph(), &other_lang()),
            Ok(SchemaValue::Option { inner: None })
        );
    }

    #[test]
    fn raw_schema_value_json_accepted_as_fallback() {
        // A raw, schema-native `SchemaValue` JSON (the tagged `kind`/`value`
        // form) is accepted when it conforms to the secret type, even though
        // the language value parser doesn't recognise it.
        let graph = graph_of(SchemaType::u32());
        let input = serde_json::to_string(&SchemaValue::U32(7)).unwrap();
        assert_eq!(
            parse_secret_value_to_schema_value(&input, &graph, &other_lang()),
            Ok(SchemaValue::U32(7))
        );
    }

    #[test]
    fn raw_schema_value_json_rejected_when_type_mismatches() {
        // The raw `SchemaValue` fallback still validates against the secret
        // type: a string value supplied for a u32 secret is rejected.
        let graph = graph_of(SchemaType::u32());
        let input = serde_json::to_string(&SchemaValue::String("x".into())).unwrap();
        let result = parse_secret_value_to_schema_value(&input, &graph, &other_lang());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Secret value does not match the expected type")
        );
    }
}
