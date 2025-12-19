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
use crate::error::HintError::NoApplicationManifestFound;
use crate::error::NonSuccessfulExit;
use crate::log::{
    log_action, log_skipping_up_to_date, log_warn_action, logln, LogColorize, LogIndent,
};
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use crate::model::text::diff::log_unified_diff;
use crate::model::text::fmt::{log_error, log_text_view};
use crate::model::text::help::EnvironmentNameHelp;
use anyhow::bail;
use golem_client::api::EnvironmentClient;
use golem_client::model::EnvironmentCreation;
use golem_common::model::application::ApplicationId;
use golem_common::model::diff;
use golem_common::model::diff::Diffable;
use golem_common::model::environment::{EnvironmentName, EnvironmentUpdate};
use std::sync::Arc;

pub struct EnvironmentCommandHandler {
    ctx: Arc<Context>,
}

impl EnvironmentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: EnvironmentSubcommand) -> anyhow::Result<()> {
        match subcommand {
            EnvironmentSubcommand::SyncDeploymentOptions => {
                self.cmd_sync_deployment_options().await
            }
        }
    }

    async fn cmd_sync_deployment_options(&self) -> anyhow::Result<()> {
        let environment = self
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;

        if !self
            .ensure_environment_deployment_options(&environment)
            .await?
        {
            log_skipping_up_to_date("updating environment deployment options");
        }

        Ok(())
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
                                .get_or_create_server_environment_by_manifest(
                                    &application.id,
                                    &env.environment_name,
                                )
                                .await?;
                            Ok(ResolvedEnvironmentIdentity::from_app_and_env(
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
            // NOTE: when only the env name is included in the reference
            //       AND that matches the manifest env name,
            //       then we on-demand create the application and the env
            EnvironmentReference::Environment { environment_name } => {
                let application = self
                    .ctx
                    .app_handler()
                    .get_or_create_server_application_by_manifest()
                    .await?;

                match application {
                    Some(application) => {
                        let environment = self
                            .get_or_create_server_environment_by_manifest(
                                &application.id,
                                environment_name,
                            )
                            .await?;
                        Ok(ResolvedEnvironmentIdentity::from_app_and_env(
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
                Ok(ResolvedEnvironmentIdentity::from_app_and_env(
                    Some(environment_reference),
                    application,
                    environment,
                ))
            }
            EnvironmentReference::AccountApplicationEnvironment {
                account_email,
                application_name,
                environment_name,
            } => {
                let env_summary = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment
                    .list_visible_environments(
                        Some(account_email),
                        Some(&application_name.0),
                        Some(&environment_name.0),
                    )
                    .await
                    .map_service_error()?
                    .values
                    .pop();

                match env_summary {
                    Some(env_summary) => Ok(ResolvedEnvironmentIdentity::from_summary(
                        Some(environment_reference),
                        env_summary,
                    )),
                    None => {
                        log_error(format!(
                            "Environment {} not found",
                            environment_reference.to_string().log_color_highlight()
                        ));

                        self.show_available_application_environments().await?;

                        bail!(NonSuccessfulExit);
                    }
                }
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

                self.show_available_application_environments().await?;

                bail!(NonSuccessfulExit);
            }
        }
    }

    async fn get_or_create_server_environment_by_manifest(
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
                let Some(deployment_options) = self.ctx.manifest_environment_deployment_options()
                else {
                    bail!(NoApplicationManifestFound)
                };

                self.ctx
                    .golem_clients()
                    .await?
                    .environment
                    .create_environment(
                        &application_id.0,
                        &EnvironmentCreation {
                            name: environment_name.clone(),
                            compatibility_check: deployment_options.compatibility_check(),
                            version_check: deployment_options.version_check(),
                            security_overrides: deployment_options.security_overrides(),
                        },
                    )
                    .await
                    .map_service_error()
            }
        }
    }

    // Returns true if the deployment options have been updated
    pub async fn ensure_environment_deployment_options(
        &self,
        environment: &ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<bool> {
        let Some(manifest_options) = self.ctx.manifest_environment_deployment_options() else {
            bail!(NoApplicationManifestFound)
        };

        let diffable_manifest_options = manifest_options.to_diffable();
        let diffable_current_options = environment.server_environment.to_diffable();

        let Some(_diff) = diffable_manifest_options.diff_with_current(&diffable_current_options)
        else {
            return Ok(false);
        };

        let unified_diff = diffable_manifest_options.unified_yaml_diff_with_current(
            &diffable_current_options,
            diff::SerializeMode::ValueIfAvailable,
        );

        log_warn_action("Detected", "environment deployment option changes");
        {
            let _indent = self.ctx.log_handler().nested_text_view_indent();
            log_unified_diff(&unified_diff);
        }
        let _indent = LogIndent::new();

        if !self
            .ctx
            .interactive_handler()
            .confirm_environment_deployment_options()?
        {
            bail!(NonSuccessfulExit);
        };

        {
            log_warn_action("Updating", "environment deployment options");
            self.ctx
                .golem_clients()
                .await?
                .environment
                .update_environment(
                    &environment.environment_id.0,
                    &EnvironmentUpdate {
                        name: None,
                        current_revision: environment.server_environment.revision,
                        compatibility_check: Some(manifest_options.compatibility_check()),
                        version_check: Some(manifest_options.version_check()),
                        security_overrides: Some(manifest_options.security_overrides()),
                    },
                )
                .await
                .map_service_error()?;
            log_action("Updated", "")
        }

        Ok(true)
    }

    fn environment_is_required_error<T>(&self, mode: EnvironmentResolveMode) -> anyhow::Result<T> {
        match mode {
            EnvironmentResolveMode::ManifestOnly => {
                log_error(
                    "The requested command requires an environment defined in an application manifest.",
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

    pub async fn show_available_application_environments(&self) -> anyhow::Result<()> {
        let _indent = LogIndent::stash();

        logln("");

        let env_summaries = self
            .ctx
            .golem_clients()
            .await?
            .environment
            .list_visible_environments(None, None, None)
            .await
            .map_service_error()?
            .values;

        if env_summaries.is_empty() {
            logln(format!(
                "No application environments are available. Use '{}' to create one.",
                "golem deploy".log_color_highlight()
            ));
        } else {
            logln(
                "Available application environments:"
                    .log_color_help_group()
                    .to_string(),
            );
            logln("");
            self.ctx.log_handler().log_view(&env_summaries);
        }

        Ok(())
    }
}
