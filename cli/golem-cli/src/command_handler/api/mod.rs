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

use crate::command::api::ApiSubcommand;
use crate::command::shared_args::DeployArgs;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::api::HttpApiDeployMode;
use crate::model::app::ApplicationComponentSelectMode;
use crate::model::component::Component;
use crate::model::{ComponentName, ProjectRefAndId};
use std::collections::BTreeMap;
use std::sync::Arc;

pub mod cloud;
pub mod definition;
pub mod deployment;
pub mod security_scheme;

pub struct ApiCommandHandler {
    ctx: Arc<Context>,
}

impl ApiCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiSubcommand) -> anyhow::Result<()> {
        match command {
            ApiSubcommand::Deploy { deploy_args } => {
                self.ctx.api_handler().cmd_deploy(deploy_args).await
            }
            ApiSubcommand::Definition { subcommand } => {
                self.ctx
                    .api_definition_handler()
                    .handle_command(subcommand)
                    .await
            }
            ApiSubcommand::Deployment { subcommand } => {
                self.ctx
                    .api_deployment_handler()
                    .handle_command(subcommand)
                    .await
            }
            ApiSubcommand::SecurityScheme { subcommand } => {
                self.ctx
                    .api_security_scheme_handler()
                    .handle_command(subcommand)
                    .await
            }
            ApiSubcommand::Cloud { subcommand } => {
                self.ctx
                    .api_cloud_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }

    pub async fn cmd_deploy(&self, deploy_args: DeployArgs) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(None)
            .await?;

        let used_component_names = {
            {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err()?;
                app_ctx
                    .application
                    .used_component_names_for_all_http_api_definition()
            }
            .into_iter()
            .map(|component_name| ComponentName::from(component_name.to_string()))
            .collect::<Vec<_>>()
        };

        let components = {
            if !used_component_names.is_empty() {
                self.ctx
                    .component_handler()
                    .deploy(
                        project.as_ref(),
                        used_component_names,
                        None,
                        &ApplicationComponentSelectMode::All,
                        &deploy_args,
                    )
                    .await?
                    .into_iter()
                    .map(|component| (component.component_name.0.clone(), component))
                    .collect::<BTreeMap<_, _>>()
            } else {
                BTreeMap::new()
            }
        };

        self.deploy(
            project.as_ref(),
            HttpApiDeployMode::All,
            &deploy_args,
            &components,
        )
        .await
    }

    pub async fn deploy(
        &self,
        project: Option<&ProjectRefAndId>,
        deploy_mode: HttpApiDeployMode,
        deploy_args: &DeployArgs,
        latest_component_versions: &BTreeMap<String, Component>,
    ) -> anyhow::Result<()> {
        if deploy_args.reset {
            self.ctx
                .api_handler()
                .delete_all_for_reset_once(project)
                .await?;
        }

        let latest_api_definition_versions = self
            .ctx
            .api_definition_handler()
            .deploy(project, deploy_mode, deploy_args, latest_component_versions)
            .await?;

        self.ctx
            .api_deployment_handler()
            .deploy(project, deploy_mode, &latest_api_definition_versions)
            .await?;

        Ok(())
    }

    pub async fn delete_all_for_reset_once(
        &self,
        project: Option<&ProjectRefAndId>,
    ) -> anyhow::Result<()> {
        self.ctx
            .api_deployment_handler()
            .delete_all_for_reset_once(project)
            .await?;

        self.ctx
            .api_definition_handler()
            .delete_all_for_reset_once(project)
            .await?;

        Ok(())
    }
}
