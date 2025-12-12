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

use crate::command::environment::EnvironmentSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{logln, LogColorize};
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use crate::model::text::fmt::{log_error, log_text_view};
use crate::model::text::help::EnvironmentNameHelp;
use anyhow::bail;
use golem_client::api::EnvironmentClient;
use golem_client::model::EnvironmentCreation;
use golem_common::model::application::ApplicationId;
use golem_common::model::environment::{EnvironmentCurrentDeploymentView, EnvironmentName};
use std::sync::Arc;

pub struct EnvironmentCommandHandler {
    ctx: Arc<Context>,
}

impl EnvironmentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, _subcommand: EnvironmentSubcommand) -> anyhow::Result<()> {
        // TODO: atomic
        todo!()
    }

    pub async fn resolve_environment(
        &self,
        mode: EnvironmentResolveMode,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        match self.ctx.environment_reference() {
            Some(environment_reference) => {
                self.resolve_environment_reference(mode, environment_reference)
                    .await
            }
            None => match self.ctx.manifest_environment() {
                Some(env) => {
                    let application = self
                        .ctx
                        .app_handler()
                        .get_or_create_server_application_by_manifest()
                        .await?;

                    match application {
                        Some(application) => {
                            let environment = self
                                .get_or_create_server_environment(
                                    &application.id,
                                    &env.environment_name,
                                )
                                .await?;
                            Ok(ResolvedEnvironmentIdentity::new(
                                None,
                                application,
                                environment,
                            ))
                        }
                        None => self.environment_is_required_error(mode)?,
                    }
                }
                None => self.environment_is_required_error(mode)?,
            },
        }
    }

    pub async fn resolve_environment_reference(
        &self,
        mode: EnvironmentResolveMode,
        environment_reference: &EnvironmentReference,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        if !mode.allowed(environment_reference) {
            self.environment_is_required_error(mode)?;
        }

        match environment_reference {
            // NOTE: when only the env name is included in the reference,
            //       we on-demand create the application and the env
            EnvironmentReference::Environment { environment_name } => {
                let application = self
                    .ctx
                    .app_handler()
                    .get_or_create_server_application_by_manifest()
                    .await?;

                match application {
                    Some(application) => {
                        let environment = self
                            .get_or_create_server_environment(&application.id, environment_name)
                            .await?;
                        Ok(ResolvedEnvironmentIdentity::new(
                            Some(environment_reference),
                            application,
                            environment,
                        ))
                    }
                    None => self.environment_is_required_error(mode)?,
                }
            }
            // NOTE: with app-env references we DO NOT create anything, these are used for
            //       querying without using the manifest
            EnvironmentReference::ApplicationEnvironment {
                application_name,
                environment_name,
            } => {
                let application = self
                    .ctx
                    .app_handler()
                    .get_server_application_or_err(&self.ctx.account_id().await?, application_name)
                    .await?;

                let environment = self
                    .get_server_environment_or_err(&application.id, environment_name)
                    .await?;
                Ok(ResolvedEnvironmentIdentity::new(
                    Some(environment_reference),
                    application,
                    environment,
                ))
            }
            EnvironmentReference::AccountApplicationEnvironment { .. } => {
                // TODO: atomic: use search / lookup API once available
                // TODO: this mode should be dynamic on auto-creation based on the current account id
                //       and the use case
                todo!()
            }
        }
    }

    pub async fn resolve_opt_environment_reference(
        &self,
        mode: EnvironmentResolveMode,
        environment_reference: Option<&EnvironmentReference>,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        match &environment_reference {
            Some(environment_reference) => {
                self.ctx
                    .environment_handler()
                    .resolve_environment_reference(mode, environment_reference)
                    .await
            }
            None => {
                self.ctx
                    .environment_handler()
                    .resolve_environment(mode)
                    .await
            }
        }
    }

    async fn get_server_environment(
        &self,
        application_id: &ApplicationId,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<Option<golem_client::model::Environment>> {
        self.ctx
            .golem_clients()
            .await?
            .environment
            .get_application_environment(&application_id.0, &environment_name.0)
            .await
            .map_service_error_not_found_as_opt()
    }

    async fn get_server_environment_or_err(
        &self,
        application_id: &ApplicationId,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<golem_client::model::Environment> {
        match self
            .get_server_environment(application_id, environment_name)
            .await?
        {
            Some(environment) => Ok(environment),
            None => {
                log_error(format!(
                    "Environment {} not found",
                    environment_name.0.log_color_highlight()
                ));
                bail!(NonSuccessfulExit);
            }
        }
    }

    async fn get_or_create_server_environment(
        &self,
        application_id: &ApplicationId,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<golem_client::model::Environment> {
        match self
            .get_server_environment(application_id, environment_name)
            .await?
        {
            Some(environment) => Ok(environment),
            None => self
                .ctx
                .golem_clients()
                .await?
                .environment
                .create_environment(
                    &application_id.0,
                    &EnvironmentCreation {
                        name: environment_name.clone(),
                        // TODO: atomic: get props from manifest
                        compatibility_check: false,
                        version_check: false,
                        security_overrides: false,
                    },
                )
                .await
                .map_service_error(),
        }
    }

    pub fn resolved_current_deployment<'a>(
        &self,
        environment: &'a ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<&'a EnvironmentCurrentDeploymentView> {
        match environment.remote_environment.current_deployment.as_ref() {
            Some(deployment) => Ok(deployment),
            None => {
                log_error(format!(
                    "No deployment found for {}",
                    environment.text_format()
                ));
                bail!(NonSuccessfulExit);
            }
        }
    }

    fn environment_is_required_error<T>(&self, mode: EnvironmentResolveMode) -> anyhow::Result<T> {
        match mode {
            EnvironmentResolveMode::ManifestOnly => {
                log_error(
                    "The requested command requires an environment from an application manifest.",
                );
            }
            EnvironmentResolveMode::Any => {
                log_error("The requested command requires an environment from an application manifest or via flags or environment variables.");
            }
        }
        logln("");
        log_text_view(&EnvironmentNameHelp);
        bail!(NonSuccessfulExit);
    }
}
