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

use crate::agent_id_display::{SourceLanguage, parse_type_for_language};
use crate::command::api::agent_secret::AgentSecretSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::AnyhowMapServiceError;
use crate::log::log_error;
use crate::model::GuestLanguage;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::text::agent_secret::{
    AgentSecretCreateView, AgentSecretDeleteView, AgentSecretGetView, AgentSecretListView,
    AgentSecretUpdateView,
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
use golem_wasm::parse_value_and_type;
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct AgentSecretCommandHandler {
    ctx: Arc<Context>,
}

impl AgentSecretCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: AgentSecretSubcommand) -> anyhow::Result<()> {
        match command {
            AgentSecretSubcommand::Create {
                path,
                secret_type,
                secret_value,
            } => self.cmd_create(path, secret_type, secret_value).await,
            AgentSecretSubcommand::Get { path, id } => self.cmd_get(path, id).await,
            AgentSecretSubcommand::UpdateValue {
                path,
                id,
                secret_value,
            } => self.cmd_update_value(path, id, secret_value).await,
            AgentSecretSubcommand::Delete { path, id } => self.cmd_delete(path, id).await,
            AgentSecretSubcommand::List => self.cmd_list().await,
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
            log_error("Either path or id must be provided".to_string());
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
                                log_error(format!(
                                    "Malformed secret type provided: {json_err}"
                                ));
                                bail!(NonSuccessfulExit);
                            }
                        },
                    }
                }
            };

        let secret_value: Option<serde_json::Value> = match secret_value {
            Some(sv) => Some(self.parse_secret_value(&sv, &secret_type)?),
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
            .log_view(&AgentSecretCreateView {
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

        self.ctx
            .log_handler()
            .log_view(&AgentSecretGetView {
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

        let clients = self.ctx.golem_clients().await?;

        let secret_value: Option<serde_json::Value> = match secret_value {
            Some(sv) => Some(self.parse_secret_value(&sv, &current.secret_type)?),
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
            .log_view(&AgentSecretUpdateView {
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

        self.ctx
            .log_handler()
            .log_view(&AgentSecretDeleteView {
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
                };
            }
        }
        SourceLanguage::Other(String::new())
    }

    fn parse_secret_value(
        &self,
        input: &str,
        secret_type: &AnalysedType,
    ) -> anyhow::Result<serde_json::Value> {
        // Try WAVE format first
        if let Ok(vat) = parse_value_and_type(secret_type, input) {
            if let Ok(json) = vat.to_json_value() {
                return Ok(json);
            }
        }
        // Fall back to raw JSON
        match serde_json::from_str(input) {
            Ok(json) => Ok(json),
            Err(err) => {
                log_error(format!("Secret value is not valid: {err}"));
                bail!(NonSuccessfulExit);
            }
        }
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
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

        self.ctx.log_handler().log_view(&AgentSecretListView {
            show_sensitive: self.ctx.show_sensitive(),
            secrets: results,
        });

        Ok(())
    }
}
