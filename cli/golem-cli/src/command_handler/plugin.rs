// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::command::plugin::PluginSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::plugin_manifest::{PluginManifest, PluginTypeSpecificManifest};
use crate::model::text::plugin::{PluginRegistrationGetView, PluginRegistrationRegisterView};
use crate::model::PathBufOrStdin;
use anyhow::{anyhow, Context as AnyhowContext};
use golem_client::api::PluginClient;
use golem_client::model::PluginRegistrationCreation;
use golem_common::model::base64::Base64;
use golem_common::model::plugin_registration::{OplogProcessorPluginSpec, PluginSpecDto};
use std::sync::Arc;
use uuid::Uuid;

pub struct PluginCommandHandler {
    ctx: Arc<Context>,
}

impl PluginCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: PluginSubcommand) -> anyhow::Result<()> {
        match subcommand {
            PluginSubcommand::List => self.cmd_list().await,
            PluginSubcommand::Get { plugin_id: id } => self.cmd_get(id).await,
            PluginSubcommand::Register { manifest } => self.cmd_register(manifest).await,
            PluginSubcommand::Unregister { plugin_id: id } => self.cmd_unregister(id).await,
        }
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let plugin_definitions = clients
            .plugin
            .get_account_plugins(&self.ctx.account_id().await?.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&plugin_definitions);

        Ok(())
    }

    async fn cmd_get(&self, id: Uuid) -> anyhow::Result<()> {
        let client = self.ctx.golem_clients().await?;

        let result = client
            .plugin
            .get_plugin_by_id(&id)
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&PluginRegistrationGetView(result));
        Ok(())
    }

    async fn cmd_register(&self, manifest: PathBufOrStdin) -> anyhow::Result<()> {
        let manifest = manifest.read_to_string()?;
        let manifest: PluginManifest = serde_yaml::from_str(&manifest)
            .with_context(|| anyhow!("Failed to decode plugin manifest"))?;

        let icon: Base64 = std::fs::read(&manifest.icon)
            .with_context(|| anyhow!("Failed to read plugin icon: {}", &manifest.icon.display()))?
            .into();

        {
            log_action(
                "Registering",
                format!(
                    "plugin {}/{}",
                    manifest.name.log_color_highlight(),
                    manifest.version.log_color_highlight()
                ),
            );

            let _indent = LogIndent::new();

            let spec = match &manifest.specs {
                PluginTypeSpecificManifest::OplogProcessor(spec) => {
                    PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                        component_id: spec.component_id.into(),
                        component_revision: spec.component_revision,
                    })
                }
            };

            let clients = self.ctx.golem_clients().await?;

            let result = clients
                .plugin
                .create_plugin(
                    &self.ctx.account_id().await?.0,
                    &PluginRegistrationCreation {
                        name: manifest.name,
                        version: manifest.version,
                        description: manifest.description,
                        icon,
                        homepage: manifest.homepage,
                        spec,
                    },
                )
                .await
                .map_service_error()?;

            self.ctx
                .log_handler()
                .log_view(&PluginRegistrationRegisterView(result));

            Ok(())
        }
    }

    async fn cmd_unregister(&self, id: Uuid) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .plugin
            .delete_plugin(&id)
            .await
            .map_service_error()?;

        log_warn_action(
            "Unregistered",
            format!(
                "plugin: {}/{}",
                result.name.log_color_highlight(),
                result.version.log_color_highlight()
            ),
        );

        Ok(())
    }
}
