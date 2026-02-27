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

use crate::app::error::CustomCommandError;
use crate::app_template::add_component_by_template;
use crate::app_template::model::{Template, TemplateName};
use crate::command::builtin_exec_subcommands;
use crate::command::exec::ExecSubcommand;
use crate::command::shared_args::{
    BuildArgs, ForceBuildArg, OptionalComponentNames, PostDeployArgs,
};
use crate::command_handler::app::deploy_diff::{
    DeployDetails, DeployDiff, DeployDiffKind, DeployQuickDiff, RollbackDetails, RollbackDiff,
    RollbackEntityDetails, RollbackQuickDiff,
};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::diagnose::diagnose;
use crate::error::service::AnyhowMapServiceError;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::fs;
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{
    log_action, log_error, log_failed_to, log_finished_ok, log_finished_up_to_date,
    log_skipping_up_to_date, log_warn, log_warn_action, logged_failed_to,
    logged_finished_or_failed_to, logln, LogColorize, LogIndent, LogOutput, Output,
};
use crate::model::app::{
    ApplicationComponentSelectMode, BuildConfig, CleanMode, DynamicHelpSections,
};
use crate::model::deploy::{
    DeployConfig, DeployError, DeployResult, DeploySummary, PostDeployError, PostDeployResult,
    PostDeploySummary,
};
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use crate::model::text::deployment::DeploymentNewView;
use crate::model::text::diff::log_unified_diff;
use crate::model::text::fmt::{log_fuzzy_matches, log_text_view};
use crate::model::text::help::AvailableComponentNamesHelp;
use crate::model::text::server::ToFormattedServerContext;
use crate::model::worker::AgentUpdateMode;
use crate::model::GuestLanguage;
use anyhow::{anyhow, bail};
use colored::Colorize;
use futures_util::{stream, StreamExt, TryStreamExt};
use golem_client::api::{ApplicationClient, ComponentClient, EnvironmentClient};
use golem_client::model::{ApplicationCreation, DeploymentCreation, DeploymentRollback};
use golem_common::model::account::AccountId;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentDto, ComponentName};
use golem_common::model::deployment::{
    CurrentDeployment, DeploymentPlanComponentEntry, DeploymentPlanHttpApiDeploymentEntry,
    DeploymentRevision, DeploymentVersion,
};
use golem_common::model::diff;
use golem_common::model::diff::{Diffable, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use strum::IntoEnumIterator;
use tracing::debug;

mod deploy_diff;

pub struct AppCommandHandler {
    ctx: Arc<Context>,
}

impl AppCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn exec_custom_command(&self, subcommand: ExecSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ExecSubcommand::CustomCommand(command) => self.cmd_custom_command(command).await,
        }
    }

    pub async fn cmd_new(
        &self,
        application_name: Option<ApplicationName>,
        languages: Vec<GuestLanguage>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.opt();
            match app_ctx {
                Ok(None) => {
                    // NOP, there is no app
                }
                _ => {
                    log_error("The current directory is part of an existing application.");
                    logln("");
                    logln("Switch to a directory that is not part of an application or use");
                    logln(format!(
                        "the '{}' command to create a component in the current application.",
                        "component new".log_color_highlight()
                    ));
                    logln("");
                    bail!(NonSuccessfulExit);
                }
            }
        }

        let Some((application_name, components)) = ({
            match application_name {
                Some(application_name) => Some((application_name, vec![])),
                None => self
                    .ctx
                    .interactive_handler()
                    .select_new_app_name_and_components()?
                    .map(|new_app| (new_app.app_name, new_app.templated_component_names)),
            }
        }) else {
            log_error("Both APPLICATION_NAME and LANGUAGES are required in non-interactive mode");
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
        };

        if components.is_empty() && languages.is_empty() {
            log_error("LANGUAGES are required in non-interactive mode");
            logln("");
            logln("Either specify languages or use the new command without APPLICATION_NAME to use the interactive wizard!");
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
        }

        let app_dir = PathBuf::from(&application_name.0);
        if app_dir.exists() {
            bail!(
                "Application directory already exists: {}",
                app_dir.log_color_error_highlight()
            );
        }

        fs::create_dir_all(&app_dir)?;
        log_action(
            "Created",
            format!(
                "application directory: {}",
                app_dir.display().to_string().log_color_highlight()
            ),
        );

        if components.is_empty() {
            let common_templates = languages
                .iter()
                .map(|language| {
                    self.get_template(language.id(), self.ctx.dev_mode())
                        .map(|(common, _component)| common)
                })
                .collect::<Result<Vec<_>, _>>()?;

            {
                let _indent = LogIndent::new();
                // TODO: cleanup add_component_by_example, so we don't have to pass a dummy arg
                let component_name = ComponentName::try_from("dummy:comp")
                    .expect("Failed to parse dummy component name.");
                for common_template in common_templates.into_iter().flatten() {
                    match add_component_by_template(
                        Some(common_template),
                        None,
                        &app_dir,
                        &application_name,
                        &component_name,
                        Some(self.ctx.template_sdk_overrides()),
                    ) {
                        Ok(()) => {
                            log_action(
                                "Added",
                                format!(
                                    "common template for {}",
                                    common_template.language.name().log_color_highlight()
                                ),
                            );
                        }
                        Err(error) => {
                            bail!("Failed to add common template for new app: {:#}", error)
                        }
                    }
                }
            }
        } else {
            for (template, component_name) in &components {
                log_action(
                    "Adding",
                    format!("component {}", component_name.0.log_color_highlight()),
                );
                let (common_template, component_template) =
                    self.get_template(template, self.ctx.dev_mode())?;
                match add_component_by_template(
                    common_template,
                    Some(component_template),
                    &app_dir,
                    &application_name,
                    component_name,
                    Some(self.ctx.template_sdk_overrides()),
                ) {
                    Ok(()) => {
                        log_action(
                            "Added",
                            format!(
                                "new app component {}",
                                component_name.0.log_color_highlight()
                            ),
                        );
                    }
                    Err(error) => {
                        bail!("Failed to create new app component: {}", error)
                    }
                }
            }
        }

        log_action(
            "Created",
            format!("application {}", application_name.0.log_color_highlight()),
        );

        logln("");

        if components.is_empty() {
            logln(
                format!(
                    "To add components to the application, switch to the {} directory, and use the `{}` command.",
                    application_name.0.log_color_highlight(),
                    "component new".log_color_highlight(),
                )
            );
        } else {
            logln(
                format!(
                    "Switch to the {} directory, and use the `{}` or `{}` commands to use your new application!",
                    application_name.0.log_color_highlight(),
                    "build".log_color_highlight(),
                    "deploy".log_color_highlight(),
                )
            );
        }

        Ok(())
    }

    pub async fn cmd_build(
        &self,
        component_name: OptionalComponentNames,
        build_args: BuildArgs,
    ) -> anyhow::Result<()> {
        let build_config = {
            let mut build_config = BuildConfig::new()
                .with_steps_filter(build_args.step.into_iter().collect())
                .with_skip_up_to_date_checks(build_args.force_build.force_build);

            if let Some(repl_bridge_sdk_target) = build_args.repl_bridge_sdk_target {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err()?;
                build_config = build_config.with_repl_bridge_sdk_target(
                    app_ctx.new_repl_bridge_sdk_target(repl_bridge_sdk_target),
                );
            }

            build_config
        };

        let result = self
            .build(
                &build_config,
                component_name.component_name,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await;

        logln("");
        logged_finished_or_failed_to(result, "building", "build application")
    }

    pub async fn cmd_clean(&self, component_name: OptionalComponentNames) -> anyhow::Result<()> {
        let result = self
            .clean(
                component_name.component_name,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await;

        logln("");
        logged_finished_or_failed_to(result, "cleaning", "clean application")
    }

    pub async fn cmd_deploy(
        &self,
        plan: bool,
        stage: bool,
        approve_staging_steps: bool,
        version: Option<String>,
        revision: Option<DeploymentRevision>,
        force_build: ForceBuildArg,
        post_deploy_args: PostDeployArgs,
        repl_bridge_sdk_target: Option<GuestLanguage>,
    ) -> anyhow::Result<()> {
        let deploy_result = {
            if let Some(version) = version {
                self.deploy_by_version(version, plan, post_deploy_args)
                    .await
            } else if let Some(revision) = revision {
                self.deploy_by_revision(revision, plan, post_deploy_args)
                    .await
            } else {
                self.deploy(DeployConfig {
                    plan,
                    stage,
                    approve_staging_steps,
                    force_build: Some(force_build),
                    post_deploy_args,
                    repl_bridge_sdk_target,
                    skip_build: false,
                })
                .await
            }
        };

        logln("");
        log_action("Summary", "");
        let _indent = LogIndent::new();

        fn logged_post_deploy_result(post_deploy_result: PostDeployResult) -> anyhow::Result<()> {
            match post_deploy_result {
                Ok(ok) => {
                    match ok {
                        PostDeploySummary::NoRequestedChanges => {
                            // NOP
                        }
                        PostDeploySummary::NoDeployment => {
                            log_warn_action(
                                "Skipped",
                                "post deployment steps, the environment has no deployment yet",
                            );
                        }
                        PostDeploySummary::AgentUpdateOk => {
                            log_finished_ok("post deployment steps, updated all agents");
                        }
                        PostDeploySummary::AgentRedeployOk => {
                            log_finished_ok("post deployment steps, redeployed all agents");
                        }
                        PostDeploySummary::AgentDeleteOk => {
                            log_finished_ok("post deployment steps, deleted all agents");
                        }
                    }
                    Ok(())
                }
                Err(err) => match err {
                    PostDeployError::PrepareError(err) => {
                        logged_failed_to(err, "prepare deployment steps")
                    }
                    PostDeployError::AgentUpdateError(err) => {
                        logged_failed_to(err, "update agents")
                    }
                    PostDeployError::AgentRedeployError(err) => {
                        logged_failed_to(err, "redeploy agents")
                    }
                    PostDeployError::AgentDeleteError(err) => {
                        logged_failed_to(err, "delete agents")
                    }
                },
            }
        }

        match deploy_result {
            Ok(ok) => match ok {
                DeploySummary::PlanOk => {
                    log_finished_ok("planning");
                    Ok(())
                }
                DeploySummary::PlanUpToDate => {
                    log_finished_up_to_date(
                        "deployment planning, no changes are required for the environment",
                    );
                    Ok(())
                }
                DeploySummary::StagingOk => {
                    log_finished_ok("staging");
                    Ok(())
                }
                DeploySummary::DeployOk(post_deploy) => {
                    log_finished_ok("deploying");
                    logged_post_deploy_result(post_deploy)
                }
                DeploySummary::DeployUpToDate(post_deploy_result) => {
                    log_finished_up_to_date(
                        "deployment planning, no changes are required for the environment",
                    );
                    logged_post_deploy_result(post_deploy_result)
                }
                DeploySummary::RollbackOk(post_deploy_result) => {
                    log_finished_ok("rollback");
                    logged_post_deploy_result(post_deploy_result)
                }
                DeploySummary::RollbackUpToDate(post_deploy_result) => {
                    log_finished_up_to_date(
                        "rollback planning, no changes are required for the environment",
                    );
                    logged_post_deploy_result(post_deploy_result)
                }
            },
            Err(err) => match err {
                DeployError::Cancelled => {
                    log_warn_action("Cancelled", "deploying");
                    Err(anyhow!(NonSuccessfulExit))
                }
                DeployError::BuildError(err) => {
                    logged_failed_to(err, "build application for deployment")
                }
                DeployError::PrepareError(err) => logged_failed_to(err, "prepare deployment"),
                DeployError::PlanError(err) => logged_failed_to(err, "plan"),
                DeployError::EnvironmentCheckError(err) => {
                    logged_failed_to(err, "check environment")
                }
                DeployError::StagingError(err) => logged_failed_to(err, "stage"),
                DeployError::DeployError(err) => logged_failed_to(err, "deploy"),
                DeployError::RollbackError(err) => {
                    log_failed_to("rollback");
                    Err(err)
                }
            },
        }
    }

    pub async fn cmd_custom_command(&self, command: Vec<String>) -> anyhow::Result<()> {
        if command.len() != 1 {
            bail!(
                "Expected exactly one custom subcommand, got: {}",
                command.join(" ").log_color_error_highlight()
            );
        }

        let command = command[0].strip_prefix(":").unwrap_or(&command[0]);

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        if let Err(error) = app_ctx.custom_command(&BuildConfig::new(), command).await {
            match error {
                CustomCommandError::CommandNotFound => {
                    logln("");
                    log_error(format!(
                        "Request command app command {} not found!",
                        command.log_color_error_highlight()
                    ));
                    logln("");

                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_custom_commands(
                        builtin_exec_subcommands(),
                    ))?;

                    logln(
                        "Available builtin commands:"
                            .log_color_help_group()
                            .to_string(),
                    );
                    let app_subcommands = builtin_exec_subcommands();
                    for subcommand in &app_subcommands {
                        logln(format!("  {}", subcommand.bold()));
                    }
                    logln("");

                    bail!(NonSuccessfulExit)
                }
                CustomCommandError::CommandError { error } => {
                    bail!(
                        "Command {} failed: {error}",
                        command.log_color_error_highlight()
                    )
                }
            }
        }

        Ok(())
    }

    pub async fn cmd_update_workers(
        &self,
        component_names: Vec<ComponentName>,
        update_mode: AgentUpdateMode,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, &ApplicationComponentSelectMode::All)
            .await?;

        let components = self.components_for_deploy_args().await?;
        self.ctx
            .component_handler()
            .update_workers_by_components(&components, update_mode, await_update, disable_wakeup)
            .await?;

        Ok(())
    }

    pub async fn cmd_redeploy_workers(
        &self,
        component_names: Vec<ComponentName>,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, &ApplicationComponentSelectMode::All)
            .await?;

        let components = self.components_for_deploy_args().await?;
        self.ctx
            .component_handler()
            .redeploy_workers_by_components(&components)
            .await?;

        Ok(())
    }

    pub async fn cmd_list_agent_types(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let agent_types = self.list_agent_types(&environment).await?;

        self.ctx.log_handler().log_view(&agent_types);

        Ok(())
    }

    pub async fn cmd_diagnose(
        &self,
        component_names: OptionalComponentNames,
    ) -> anyhow::Result<()> {
        self.diagnose(
            component_names.component_name,
            &ApplicationComponentSelectMode::All,
        )
        .await
    }

    pub async fn list_agent_types(
        &self,
        environment: &ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<Vec<DeployedRegisteredAgentType>> {
        environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    Ok(self
                        .ctx
                        .golem_clients()
                        .await?
                        .environment
                        .list_deployment_agent_types(
                            &environment.environment_id.0,
                            current_deployment_revision.into(),
                        )
                        .await
                        .map_service_error()?
                        .values)
                },
            )
            .await
    }

    async fn deploy_by_version(
        &self,
        version: String,
        plan: bool,
        post_deploy_args: PostDeployArgs,
    ) -> DeployResult {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await
            .map_err(DeployError::PrepareError)?;

        let _ = environment
            .current_deployment_or_err()
            .map_err(DeployError::PrepareError)?;

        let clients = self
            .ctx
            .golem_clients()
            .await
            .map_err(DeployError::PrepareError)?;

        let deployments = clients
            .environment
            .list_deployments(&environment.environment_id.0, Some(&version))
            .await
            .map_service_error()
            .map_err(DeployError::PrepareError)?
            .values;

        if deployments.is_empty() {
            log_error(format!(
                "Deployment with version {} not found!",
                version.log_color_error_highlight()
            ));
            self.safe_show_available_deployments(&environment).await;
            return Err(DeployError::PrepareError(anyhow!(NonSuccessfulExit)));
        } else if deployments.len() > 1 {
            log_error(format!(
                "Multiple deployment found with version {}, use deployment revision instead!",
                version.log_color_error_highlight()
            ));
            self.ctx.log_handler().log_view(&deployments);
            return Err(DeployError::PrepareError(anyhow!(NonSuccessfulExit)));
        }

        self.deploy_by_revision(
            deployments
                .first()
                .map(|d| d.revision)
                .expect("No deployments"),
            plan,
            post_deploy_args,
        )
        .await
    }

    async fn deploy_by_revision(
        &self,
        target_revision: DeploymentRevision,
        plan: bool,
        post_deploy_args: PostDeployArgs,
    ) -> DeployResult {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await
            .map_err(DeployError::PrepareError)?;

        let Some(rollback_diff) = self
            .prepare_rollback(environment.clone(), target_revision)
            .await
            .map_err(DeployError::PrepareError)?
        else {
            return {
                if plan {
                    Ok(DeploySummary::PlanUpToDate)
                } else {
                    Ok(DeploySummary::RollbackUpToDate(
                        self.apply_post_deploy_args(
                            &environment.environment_id,
                            environment
                                .server_environment
                                .current_deployment
                                .as_ref()
                                .map(|d| d.deployment_revision),
                            &post_deploy_args,
                        )
                        .await,
                    ))
                }
            };
        };

        if plan {
            return Ok(DeploySummary::PlanOk);
        }

        if !self
            .ctx
            .interactive_handler()
            .confirm_deploy_by_plan(
                &rollback_diff.environment.application_name,
                &rollback_diff.environment.environment_name,
                &self
                    .ctx
                    .manifest_environment()
                    .map(|env| env.environment.to_formatted_server_context())
                    .unwrap_or("???".to_string()),
            )
            .map_err(DeployError::PrepareError)?
        {
            return Err(DeployError::Cancelled);
        }

        self.ctx
            .environment_handler()
            .ensure_environment_deployment_options(&environment)
            .await
            .map_err(DeployError::EnvironmentCheckError)?;

        let current_deployment = self
            .rollback_environment(&rollback_diff)
            .await
            .map_err(DeployError::DeployError)?;

        Ok(DeploySummary::RollbackOk(
            self.apply_post_deploy_args(
                &current_deployment.environment_id,
                Some(current_deployment.revision),
                &post_deploy_args,
            )
            .await,
        ))
    }

    pub async fn deploy(&self, config: DeployConfig) -> DeployResult {
        let build_config = {
            let mut build_config = BuildConfig::new().with_skip_up_to_date_checks(
                config
                    .force_build
                    .as_ref()
                    .map(|f| f.force_build)
                    .unwrap_or(false),
            );

            if let Some(repl_bridge_sdk_target) = config.repl_bridge_sdk_target {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err().map_err(DeployError::PrepareError)?;
                build_config = build_config.with_repl_bridge_sdk_target(
                    app_ctx.new_repl_bridge_sdk_target(repl_bridge_sdk_target),
                );
            }

            build_config
        };

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await
            .map_err(DeployError::PrepareError)?;

        if !config.skip_build {
            self.build(&build_config, vec![], &ApplicationComponentSelectMode::All)
                .await
                .map_err(DeployError::BuildError)?;
        }

        let Some(deploy_diff) = self
            .prepare_deployment(environment.clone())
            .await
            .map_err(DeployError::PrepareError)?
        else {
            return {
                if config.plan {
                    Ok(DeploySummary::PlanUpToDate)
                } else {
                    Ok(DeploySummary::DeployUpToDate(
                        self.apply_post_deploy_args(
                            &environment.environment_id,
                            environment
                                .server_environment
                                .current_deployment
                                .as_ref()
                                .map(|d| d.deployment_revision),
                            &config.post_deploy_args,
                        )
                        .await,
                    ))
                }
            };
        };

        if config.plan {
            return Ok(DeploySummary::PlanOk);
        }

        if !self
            .ctx
            .interactive_handler()
            .confirm_deploy_by_plan(
                &deploy_diff.environment.application_name,
                &deploy_diff.environment.environment_name,
                &self
                    .ctx
                    .manifest_environment()
                    .map(|env| env.environment.to_formatted_server_context())
                    .unwrap_or("???".to_string()),
            )
            .map_err(DeployError::PrepareError)?
        {
            return Err(DeployError::Cancelled);
        }

        self.ctx
            .environment_handler()
            .ensure_environment_deployment_options(&environment)
            .await
            .map_err(DeployError::EnvironmentCheckError)?;

        self.apply_changes_to_stage(config.approve_staging_steps, &deploy_diff)
            .await
            .map_err(DeployError::StagingError)?;
        if config.stage {
            return Ok(DeploySummary::StagingOk);
        }

        let current_deployment = self
            .apply_staged_changes_to_environment(&deploy_diff)
            .await
            .map_err(DeployError::DeployError)?;

        Ok(DeploySummary::DeployOk(
            self.apply_post_deploy_args(
                &current_deployment.environment_id,
                Some(current_deployment.revision),
                &config.post_deploy_args,
            )
            .await,
        ))
    }

    async fn prepare_deployment(
        &self,
        environment: ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<Option<DeployDiff>> {
        log_action("Preparing", "deployment");
        let _indent = LogIndent::new();

        let deploy_quick_diff = self.deploy_quick_diff(environment).await?;

        debug!("deploy_quick_diff: {:#?}", deploy_quick_diff);

        if deploy_quick_diff.is_up_to_date() {
            return Ok(None);
        }

        log_action("Diffing", "");

        let deploy_diff = self.deploy_diff(deploy_quick_diff).await?;
        debug!("deploy_diff: {:#?}", deploy_diff);

        let deploy_diff = self.detailed_deploy_diff(deploy_diff).await?;
        debug!("detailed deploy_diff: {:#?}", deploy_diff);

        let unified_diffs = deploy_diff.unified_diffs(self.ctx.show_sensitive());
        let stage_is_same_as_current = deploy_diff.is_stage_same_as_current();

        {
            let _indent = LogIndent::new();

            log_action(
                "Comparing",
                format!(
                    "staging area with current deployment: {}",
                    if stage_is_same_as_current {
                        "SAME".green()
                    } else {
                        "DIFFERENT".yellow()
                    }
                ),
            );

            if !stage_is_same_as_current {
                match &unified_diffs.deployment_diff_stage {
                    Some(diff) => {
                        log_action("Diffing", "with staging area");
                        let _indent = self.ctx.log_handler().nested_text_view_indent();
                        log_unified_diff(diff);
                        if let Some(diff) = unified_diffs.agent_diff_stage {
                            logln("");
                            log_unified_diff(&diff);
                        }
                    }
                    None => {
                        log_skipping_up_to_date("diffing with staging area");
                    }
                }
            }

            {
                if stage_is_same_as_current {
                    log_action("Diffing", "with staging area and current deployment");
                } else {
                    log_action("Diffing", "with current deployment");
                }

                let _indent = self.ctx.log_handler().nested_text_view_indent();
                log_unified_diff(&unified_diffs.deployment_diff);
                if let Some(diff) = unified_diffs.agent_diff {
                    logln("");
                    log_unified_diff(&diff);
                }
            }
        }

        {
            log_action("Planning", "");
            let _indent = LogIndent::new();

            if !stage_is_same_as_current {
                match &deploy_diff.diff_stage {
                    Some(diff_stage) => {
                        log_action("Planned", "changes to be applied to the staging area:");
                        let _indent = self.ctx.log_handler().nested_text_view_indent();
                        self.ctx.log_handler().log_view(diff_stage)
                    }
                    None => log_skipping_up_to_date("planning changes for staging area"),
                }
            }

            {
                if stage_is_same_as_current {
                    log_action(
                        "Planned",
                        "changes to be applied to the staging area and to the environment:",
                    );
                } else {
                    log_action("Planned", "changes to be applied to the environment:");
                }
                let _indent = self.ctx.log_handler().nested_text_view_indent();
                self.ctx.log_handler().log_view(&deploy_diff.diff)
            }
        }

        Ok(Some(deploy_diff))
    }

    async fn deploy_quick_diff(
        &self,
        environment: ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<DeployQuickDiff> {
        let deployable_manifest_components = self
            .ctx
            .component_handler()
            .deployable_manifest_components()
            .await?;

        let deployable_manifest_http_api_deployments = self
            .ctx
            .api_deployment_handler()
            .deployable_manifest_api_deployments(&environment.environment_name)
            .await?;

        let deployable_manifest_mcp_deployments = self
            .ctx
            .api_deployment_handler()
            .deployable_manifest_mcp_deployments(&environment.environment_name)
            .await?;

        let diffable_local_components = {
            let mut diffable_components = BTreeMap::<String, diff::HashOf<diff::Component>>::new();
            for (component_name, component_deploy_properties) in &deployable_manifest_components {
                let diffable_component = self
                    .ctx
                    .component_handler()
                    .diffable_local_component(
                        &environment,
                        component_name,
                        component_deploy_properties,
                    )
                    .await?;
                diffable_components.insert(component_name.0.clone(), diffable_component.into());
            }
            diffable_components
        };

        let diffable_local_http_api_deployments = {
            let mut diffable_local_http_api_deployments =
                BTreeMap::<String, diff::HashOf<diff::HttpApiDeployment>>::new();
            for (domain, http_api_deployment) in &deployable_manifest_http_api_deployments {
                let agents = http_api_deployment
                    .agents
                    .iter()
                    .map(|(k, v)| (k.0.clone(), v.to_diffable()))
                    .collect();

                diffable_local_http_api_deployments.insert(
                    domain.0.clone(),
                    diff::HttpApiDeployment {
                        webhooks_url: http_api_deployment.webhooks_url.clone(),
                        agents,
                    }
                    .into(),
                );
            }
            diffable_local_http_api_deployments
        };

        let diffable_local_mcp_deployments = {
            let mut diffable_local_mcp_deployments =
                BTreeMap::<String, diff::HashOf<diff::McpDeployment>>::new();
            for (domain, mcp_deployment) in &deployable_manifest_mcp_deployments {
                let agents = mcp_deployment
                    .agents
                    .iter()
                    .map(|(k, v)| (k.0.clone(), v.to_diffable()))
                    .collect();
                diffable_local_mcp_deployments
                    .insert(domain.0.clone(), diff::McpDeployment { agents }.into());
            }
            diffable_local_mcp_deployments
        };

        let diffable_local_deployment = diff::Deployment {
            components: diffable_local_components,
            http_api_deployments: diffable_local_http_api_deployments,
            mcp_deployments: diffable_local_mcp_deployments,
        };

        let local_deployment_hash = diffable_local_deployment.hash();

        Ok(DeployQuickDiff {
            environment,
            deployable_manifest_components,
            deployable_manifest_http_api_deployments,
            deployable_manifest_mcp_deployments,
            diffable_local_deployment,
            local_deployment_hash,
        })
    }

    async fn deploy_diff(&self, deploy_quick_diff: DeployQuickDiff) -> anyhow::Result<DeployDiff> {
        let clients = self.ctx.golem_clients().await?;

        let current_deployment = match &deploy_quick_diff
            .environment
            .server_environment
            .current_deployment
        {
            Some(current_deployment) => Some(
                clients
                    .environment
                    .get_deployment_summary(
                        &deploy_quick_diff.environment.environment_id.0,
                        current_deployment.deployment_revision.into(),
                    )
                    .await
                    .map_service_error()?,
            ),
            None => None,
        };

        let diffable_current_deployment = current_deployment
            .as_ref()
            .map(|d| d.to_diffable())
            .unwrap_or_default();

        let current_deployment_hash = diffable_current_deployment.hash();

        let staged_deployment = clients
            .environment
            .get_environment_deployment_plan(&deploy_quick_diff.environment.environment_id.0)
            .await
            .map_service_error()?;

        let diffable_staged_deployment = staged_deployment.to_diffable();

        let staged_deployment_hash = diffable_staged_deployment.hash();

        let Some(diff) =
            diffable_current_deployment.diff_with_new(&deploy_quick_diff.diffable_local_deployment)
        else {
            bail!(anyhow!("The environment was changed concurrently while diffing. Retry planning and deploying!"))
        };

        let diff_stage =
            diffable_staged_deployment.diff_with_new(&deploy_quick_diff.diffable_local_deployment);

        Ok(DeployDiff {
            environment: deploy_quick_diff.environment,
            deployable_components: deploy_quick_diff.deployable_manifest_components,
            deployable_http_api_deployments: deploy_quick_diff
                .deployable_manifest_http_api_deployments,
            deployable_mcp_deployments: deploy_quick_diff.deployable_manifest_mcp_deployments,
            diffable_local_deployment: deploy_quick_diff.diffable_local_deployment,
            local_deployment_hash: deploy_quick_diff.local_deployment_hash,
            current_deployment,
            diffable_current_deployment,
            current_deployment_hash,
            current_agent_types: HashMap::new(),
            staged_deployment,
            staged_deployment_hash,
            staged_agent_types: HashMap::new(),
            diffable_staged_deployment,
            diff,
            diff_stage,
        })
    }

    async fn detailed_deploy_diff(
        &self,
        mut deploy_diff: DeployDiff,
    ) -> anyhow::Result<DeployDiff> {
        let parallelism = self.ctx.http_parallelism();
        let limiter = Arc::new(tokio::sync::Semaphore::new(parallelism));

        for kind in [DeployDiffKind::Stage, DeployDiffKind::Current] {
            if let Some(details) = self
                .collect_deploy_diff_details(kind, parallelism, limiter.clone(), &deploy_diff)
                .await?
            {
                deploy_diff.add_details(kind, details)?;
            }
        }

        debug!(
            "diffable_server_staged_deployment hash: {:#?}",
            deploy_diff.diffable_staged_deployment.hash()
        );

        Ok(deploy_diff)
    }

    async fn collect_deploy_diff_details(
        &self,
        kind: DeployDiffKind,
        parallelism: usize,
        limiter: Arc<tokio::sync::Semaphore>,
        deploy_diff: &DeployDiff,
    ) -> anyhow::Result<Option<DeployDetails>> {
        let diff = match kind {
            DeployDiffKind::Stage => match deploy_diff.diff_stage.as_ref() {
                Some(diff) => diff,
                None => {
                    return Ok(None);
                }
            },
            DeployDiffKind::Current => &deploy_diff.diff,
        };

        let component_handler = self.ctx.component_handler();
        let http_api_deployment_handler = self.ctx.api_deployment_handler();

        let component_details = stream::iter(diff.components.iter().filter_map(
            |(component_name, component_diff)| {
                match component_diff {
                    diff::BTreeMapDiffValue::Create => None,
                    diff::BTreeMapDiffValue::Update(_) | diff::BTreeMapDiffValue::Delete => {
                        // NOTE: Unlike other entities, for components we also fetch
                        //       details for deletes (not just for updates),
                        //       so we can show agent type diffs too
                        Some(component_name)
                    }
                }
            },
        ))
        .map(|component_name| {
            let component_name = ComponentName(component_name.clone());
            let component_identity = deploy_diff.component_identity(kind, &component_name);
            async {
                let _permit = limiter.acquire().await?;
                let component = component_handler
                    .get_component_revision_by_id(
                        &component_identity.id,
                        component_identity.revision,
                    )
                    .await?;
                Ok::<_, anyhow::Error>((component_name, component))
            }
        })
        .buffer_unordered(parallelism)
        .try_collect::<Vec<_>>();

        let http_api_deployment_details =
            stream::iter(diff.http_api_deployments.iter().filter_map(
                |(domain, http_api_deployment_diff)| match http_api_deployment_diff {
                    diff::BTreeMapDiffValue::Create | diff::BTreeMapDiffValue::Delete => None,
                    diff::BTreeMapDiffValue::Update(_) => Some(domain),
                },
            ))
            .map(|domain| {
                let domain = Domain(domain.clone());
                let deployment_identity = deploy_diff.http_api_deployment_identity(kind, &domain);
                async {
                    let _permit = limiter.acquire().await?;
                    let deployment = http_api_deployment_handler
                        .get_http_api_deployment_revision_by_id(
                            &deployment_identity.id,
                            deployment_identity.revision,
                        )
                        .await?;
                    Ok::<_, anyhow::Error>((domain, deployment))
                }
            })
            .buffer_unordered(parallelism)
            .try_collect::<Vec<_>>();

        let (component, http_api_deployment) =
            tokio::try_join!(component_details, http_api_deployment_details,)?;

        Ok(Some(DeployDetails {
            component,
            http_api_deployment,
        }))
    }

    async fn prepare_rollback(
        &self,
        environment: ResolvedEnvironmentIdentity,
        deployment_revision: DeploymentRevision,
    ) -> anyhow::Result<Option<RollbackDiff>> {
        log_action("Preparing", "rollback");
        let _indent = LogIndent::new();

        log_action("Diffing", "current deployment with target revision");

        let rollback_quick_diff = self
            .rollback_quick_diff(environment, deployment_revision)
            .await?;
        debug!("rollback_quick_diff: {:#?}", rollback_quick_diff);

        if rollback_quick_diff.is_target_same_as_current() {
            return Ok(None);
        }

        let rollback_diff = self.rollback_diff(rollback_quick_diff).await?;
        debug!("rollback_diff: {:#?}", rollback_diff);

        let rollback_diff = self.detailed_rollback_diff(rollback_diff).await?;
        debug!("detailed rollback_diff: {:#?}", rollback_diff);

        let unified_diffs = rollback_diff.unified_diffs(self.ctx.show_sensitive());

        {
            let _indent = self.ctx.log_handler().nested_text_view_indent();
            log_unified_diff(&unified_diffs.deployment_diff);
            if let Some(diff) = unified_diffs.agent_diff {
                logln("");
                log_unified_diff(&diff);
            }
        }

        {
            log_action("Planned", "changes to be applied to the environment:");
            let _indent = self.ctx.log_handler().nested_text_view_indent();
            self.ctx.log_handler().log_view(&rollback_diff.diff)
        }

        Ok(Some(rollback_diff))
    }

    async fn rollback_quick_diff(
        &self,
        environment: ResolvedEnvironmentIdentity,
        deployment_revision: DeploymentRevision,
    ) -> anyhow::Result<RollbackQuickDiff> {
        let clients = self.ctx.golem_clients().await?;
        let current_deployment_meta = environment.current_deployment_or_err()?.clone();

        let Some(target_deployment) = clients
            .environment
            .get_deployment_summary(&environment.environment_id.0, deployment_revision.get())
            .await
            .map_service_error_not_found_as_opt()?
        else {
            let _indent = LogIndent::stash();

            log_error(format!(
                "Deployment revision {} not found",
                deployment_revision.get().to_string().log_color_highlight(),
            ));
            self.show_available_deployments(&environment).await?;
            bail!(NonSuccessfulExit);
        };

        Ok(RollbackQuickDiff {
            environment,
            current_deployment_meta,
            target_deployment,
        })
    }

    async fn rollback_diff(&self, quick_diff: RollbackQuickDiff) -> anyhow::Result<RollbackDiff> {
        let clients = self.ctx.golem_clients().await?;

        let current_deployment = clients
            .environment
            .get_deployment_summary(
                &quick_diff.environment.environment_id.0,
                quick_diff.current_deployment_meta.deployment_revision.get(),
            )
            .await
            .map_service_error()?;

        let diffable_target_deployment = quick_diff.target_deployment.to_diffable();
        let diffable_current_deployment = current_deployment.to_diffable();

        let Some(diff) = diffable_target_deployment.diff_with_current(&diffable_current_deployment)
        else {
            bail!("Illegal state: empty diff between current and target deployment after fetching summaries")
        };

        Ok(RollbackDiff {
            environment: quick_diff.environment,
            current_deployment_meta: quick_diff.current_deployment_meta,
            target_deployment: quick_diff.target_deployment,
            current_deployment,
            diffable_target_deployment,
            diffable_current_deployment,
            current_agent_types: HashMap::new(),
            target_agent_types: HashMap::new(),
            diff,
        })
    }

    async fn detailed_rollback_diff(
        &self,
        mut rollback_diff: RollbackDiff,
    ) -> anyhow::Result<RollbackDiff> {
        let parallelism = self.ctx.http_parallelism();
        let limiter = Arc::new(tokio::sync::Semaphore::new(parallelism));

        let component_details = stream::iter(rollback_diff.diff.components.iter().map(
            |(component_name, component_diff)| {
                RollbackEntityDetails::new_identity(
                    ComponentName(component_name.clone()),
                    RollbackDiff::target_component_identity,
                    RollbackDiff::current_component_identity,
                    &rollback_diff,
                    component_diff,
                )
            },
        ))
        .map(|details| {
            let ctx = self.ctx.clone();
            let limiter = limiter.clone();
            async move {
                let get = async |identity: Option<&DeploymentPlanComponentEntry>| match identity {
                    Some(identity) => {
                        let _permit = limiter.acquire().await?;
                        Ok::<_, anyhow::Error>(Some(
                            ctx.component_handler()
                                .get_component_revision_by_id(&identity.id, identity.revision)
                                .await?,
                        ))
                    }
                    None => Ok::<_, anyhow::Error>(None),
                };

                Ok::<_, anyhow::Error>(RollbackEntityDetails {
                    name: details.name,
                    new: get(details.new).await?,
                    current: get(details.current).await?,
                })
            }
        })
        .buffer_unordered(parallelism)
        .try_collect::<Vec<_>>();

        let http_api_deployment_details =
            stream::iter(rollback_diff.diff.http_api_deployments.iter().map(
                |(domain, http_api_deployment_diff)| {
                    RollbackEntityDetails::new_identity(
                        Domain(domain.clone()),
                        RollbackDiff::target_http_api_deployment_identity,
                        RollbackDiff::current_http_api_deployment_identity,
                        &rollback_diff,
                        http_api_deployment_diff,
                    )
                },
            ))
            .map(|details| {
                let ctx = self.ctx.clone();
                let limiter = limiter.clone();
                async move {
                    let get = async |identity: Option<&DeploymentPlanHttpApiDeploymentEntry>| {
                        match identity {
                            Some(identity) => {
                                let _permit = limiter.acquire().await?;
                                Ok::<_, anyhow::Error>(Some(
                                    ctx.api_deployment_handler()
                                        .get_http_api_deployment_revision_by_id(
                                            &identity.id,
                                            identity.revision,
                                        )
                                        .await?,
                                ))
                            }
                            None => Ok::<_, anyhow::Error>(None),
                        }
                    };

                    Ok::<_, anyhow::Error>(RollbackEntityDetails {
                        name: details.name,
                        new: get(details.new).await?,
                        current: get(details.current).await?,
                    })
                }
            })
            .buffer_unordered(parallelism)
            .try_collect::<Vec<_>>();

        let (component, http_api_deployment) =
            tokio::try_join!(component_details, http_api_deployment_details,)?;

        rollback_diff.add_details(RollbackDetails {
            component,
            http_api_deployment,
        })?;

        Ok(rollback_diff)
    }

    async fn apply_changes_to_stage(
        &self,
        approve_staging_steps: bool,
        deploy_diff: &DeployDiff,
    ) -> anyhow::Result<()> {
        let Some(diff_stage) = &deploy_diff.diff_stage else {
            log_skipping_up_to_date("changing staging area");
            return Ok(());
        };

        log_action("Applying", "changes to the staging area");
        let _indent = LogIndent::new();

        let component_handler = self.ctx.component_handler();
        let http_api_deployment_handler = self.ctx.api_deployment_handler();
        let interactive_handler = self.ctx.interactive_handler();

        let approve = || {
            if approve_staging_steps && !interactive_handler.confirm_staging_next_step()? {
                bail!("Aborted staging");
            }
            Ok(())
        };

        // TODO
        for (component_name, component_diff) in &diff_stage.components {
            approve()?;

            let component_name = ComponentName(component_name.to_string());

            match component_diff {
                diff::BTreeMapDiffValue::Create => {
                    component_handler
                        .create_staged_component(
                            &deploy_diff.environment,
                            &component_name,
                            deploy_diff.deployable_manifest_component(&component_name),
                        )
                        .await?
                }
                diff::BTreeMapDiffValue::Delete => {
                    component_handler
                        .delete_staged_component(
                            deploy_diff.staged_component_identity(&component_name),
                        )
                        .await?
                }
                diff::BTreeMapDiffValue::Update(component_diff) => {
                    component_handler
                        .update_staged_component(
                            &deploy_diff.environment,
                            deploy_diff.staged_component_identity(&component_name),
                            deploy_diff.deployable_manifest_component(&component_name),
                            component_diff,
                        )
                        .await?
                }
            }
        }

        for (domain, http_api_deployment_diff) in &diff_stage.http_api_deployments {
            approve()?;

            let domain = Domain(domain.to_string());

            match http_api_deployment_diff {
                diff::BTreeMapDiffValue::Create => {
                    http_api_deployment_handler
                        .create_staged_http_api_deployment(
                            &deploy_diff.environment,
                            &domain,
                            deploy_diff.deployable_manifest_http_api_deployment(&domain),
                        )
                        .await?
                }
                diff::BTreeMapDiffValue::Delete => {
                    http_api_deployment_handler
                        .delete_staged_http_api_deployment(
                            deploy_diff.staged_http_api_deployment_identity(&domain),
                        )
                        .await?
                }
                diff::BTreeMapDiffValue::Update(http_api_definition_diff) => {
                    http_api_deployment_handler
                        .update_staged_http_api_deployment(
                            deploy_diff.staged_http_api_deployment_identity(&domain),
                            deploy_diff.deployable_manifest_http_api_deployment(&domain),
                            http_api_definition_diff,
                        )
                        .await?
                }
            }
        }

        for (domain, mcp_deployment_diff) in &diff_stage.mcp_deployments {
            approve()?;

            let domain = Domain(domain.to_string());

            match mcp_deployment_diff {
                diff::BTreeMapDiffValue::Create => {
                    let mcp_deployment_handler = self.ctx.api_deployment_handler();
                    mcp_deployment_handler
                        .create_staged_mcp_deployment(
                            &deploy_diff.environment,
                            &domain,
                            deploy_diff.deployable_manifest_mcp_deployment(&domain),
                        )
                        .await?
                }
                diff::BTreeMapDiffValue::Delete => {
                    let mcp_deployment_handler = self.ctx.api_deployment_handler();
                    mcp_deployment_handler
                        .delete_staged_mcp_deployment(
                            deploy_diff.staged_mcp_deployment_identity(&domain),
                        )
                        .await?
                }
                diff::BTreeMapDiffValue::Update(mcp_deployment_diff) => {
                    let mcp_deployment_handler = self.ctx.api_deployment_handler();
                    let mcp_deployment = deploy_diff.deployable_manifest_mcp_deployment(&domain);
                    let agents = mcp_deployment
                        .agents.keys().map(|k| (k.clone(), golem_common::model::mcp_deployment::McpDeploymentAgentOptions::default()))
                        .collect();

                    mcp_deployment_handler
                        .update_staged_mcp_deployment(
                            deploy_diff.staged_mcp_deployment_identity(&domain),
                            &golem_common::model::mcp_deployment::McpDeploymentUpdate {
                                current_revision: deploy_diff
                                    .staged_mcp_deployment_identity(&domain)
                                    .revision,
                                domain: Some(domain.clone()),
                                agents: Some(agents),
                            },
                            mcp_deployment_diff,
                        )
                        .await?
                }
            }
        }

        Ok(())
    }

    async fn apply_staged_changes_to_environment(
        &self,
        deploy_diff: &DeployDiff,
    ) -> anyhow::Result<CurrentDeployment> {
        let clients = self.ctx.golem_clients().await?;

        log_action("Deploying", "staged changes to the environment");

        let result = clients
            .environment
            .deploy_environment(
                &deploy_diff.environment.environment_id.0,
                &DeploymentCreation {
                    current_revision: deploy_diff.current_deployment_revision(),
                    expected_deployment_hash: deploy_diff.local_deployment_hash,
                    version: DeploymentVersion("".to_string()), // TODO: atomic
                },
            )
            .await
            .map_service_error()?;

        log_action("Deployed", "all changes");

        self.ctx.log_handler().log_view(&DeploymentNewView {
            application_name: deploy_diff.environment.application_name.clone(),
            environment_name: deploy_diff.environment.environment_name.clone(),
            deployment: result.clone(),
        });

        Ok(result)
    }

    async fn rollback_environment(
        &self,
        rollback_diff: &RollbackDiff,
    ) -> anyhow::Result<CurrentDeployment> {
        let clients = self.ctx.golem_clients().await?;

        log_warn_action(
            "Rolling back",
            format!(
                "environment to revision {}",
                rollback_diff
                    .target_deployment
                    .deployment_revision
                    .get()
                    .to_string()
                    .log_color_highlight()
            ),
        );

        let result = clients
            .environment
            .rollback_environment(
                &rollback_diff.environment.environment_id.0,
                &DeploymentRollback {
                    current_revision: rollback_diff.current_deployment_meta.revision,
                    deployment_revision: rollback_diff.target_deployment.deployment_revision,
                },
            )
            .await
            .map_service_error()?;

        log_action("Rolled back", "all changes");

        self.ctx.log_handler().log_view(&DeploymentNewView {
            application_name: rollback_diff.environment.application_name.clone(),
            environment_name: rollback_diff.environment.environment_name.clone(),
            deployment: result.clone(),
        });

        Ok(result)
    }

    async fn apply_post_deploy_args(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: Option<DeploymentRevision>,
        post_deploy_args: &PostDeployArgs,
    ) -> PostDeployResult {
        let Some(deployment_revision) = deployment_revision else {
            return Ok(PostDeploySummary::NoDeployment);
        };

        if !post_deploy_args.is_any_set(self.ctx.deploy_args()) {
            return Ok(PostDeploySummary::NoRequestedChanges);
        }

        let env_deploy_args = self.ctx.deploy_args();

        let components = self
            .ctx
            .golem_clients()
            .await
            .map_err(PostDeployError::PrepareError)?
            .environment
            .get_deployment_components(&environment_id.0, deployment_revision.into())
            .await
            .map_service_error()
            .map_err(PostDeployError::PrepareError)?
            .values;

        if let Some(update_mode) = &post_deploy_args.update_agents {
            self.ctx
                .component_handler()
                .update_workers_by_components(&components, *update_mode, true, false)
                .await
                .map(|()| PostDeploySummary::AgentUpdateOk)
                .map_err(PostDeployError::AgentUpdateError)
        } else if post_deploy_args.redeploy_agents(env_deploy_args) {
            self.ctx
                .component_handler()
                .redeploy_workers_by_components(&components)
                .await
                .map(|()| PostDeploySummary::AgentRedeployOk)
                .map_err(PostDeployError::AgentRedeployError)
        } else if post_deploy_args.delete_agents(env_deploy_args) {
            self.ctx
                .component_handler()
                .delete_workers(&components)
                .await
                .map(|()| PostDeploySummary::AgentDeleteOk)
                .map_err(PostDeployError::AgentDeleteError)
        } else {
            Ok(PostDeploySummary::NoRequestedChanges)
        }
    }

    pub async fn get_server_application(
        &self,
        account_id: &AccountId,
        application_name: &ApplicationName,
    ) -> anyhow::Result<Option<golem_client::model::Application>> {
        self.ctx
            .golem_clients()
            .await?
            .application
            .get_account_application(&account_id.0, &application_name.0)
            .await
            .map_service_error_not_found_as_opt()
    }

    pub async fn get_server_application_or_err(
        &self,
        account_id: &AccountId,
        application_name: &ApplicationName,
    ) -> anyhow::Result<golem_client::model::Application> {
        match self
            .get_server_application(account_id, application_name)
            .await?
        {
            Some(application) => Ok(application),
            None => {
                log_error(format!(
                    "Application {} not found",
                    application_name.0.log_color_highlight()
                ));

                self.ctx
                    .environment_handler()
                    .show_available_application_environments()
                    .await?;

                bail!(NonSuccessfulExit)
            }
        }
    }

    pub async fn get_or_create_server_application_by_manifest(
        &self,
    ) -> anyhow::Result<Option<golem_client::model::Application>> {
        let Some(application_name) = self.ctx.manifest_environment().map(|e| &e.application_name)
        else {
            return Ok(None);
        };

        let account_id = self.ctx.account_id().await?;

        match self
            .get_server_application(&account_id, application_name)
            .await?
        {
            Some(application) => Ok(Some(application)),
            None => Ok(Some(
                self.ctx
                    .golem_clients()
                    .await?
                    .application
                    .create_application(
                        &account_id.0,
                        &ApplicationCreation {
                            name: application_name.clone(),
                        },
                    )
                    .await
                    .map_service_error()?,
            )),
        }
    }

    pub async fn build(
        &self,
        build_config: &BuildConfig,
        component_names: Vec<ComponentName>,
        default_component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, default_component_select_mode)
            .await?;
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        app_ctx.build(build_config).await
    }

    pub async fn clean(
        &self,
        component_names: Vec<ComponentName>,
        default_component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        let any_component_requested = !component_names.is_empty();

        self.must_select_components(component_names, default_component_select_mode)
            .await?;

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let all_selected =
            app_ctx.selected_component_names().len() == app_ctx.application().component_count();

        let clean_mode = {
            if all_selected {
                CleanMode::All
            } else if any_component_requested {
                CleanMode::SelectedComponentsOnly
            } else {
                match &default_component_select_mode {
                    ApplicationComponentSelectMode::CurrentDir
                    | ApplicationComponentSelectMode::Explicit(_) => {
                        CleanMode::SelectedComponentsOnly
                    }
                    ApplicationComponentSelectMode::All => CleanMode::All,
                }
            }
        };

        app_ctx.clean(&BuildConfig::new(), clean_mode)
    }

    async fn components_for_deploy_args(&self) -> anyhow::Result<Vec<ComponentDto>> {
        let clients = self.ctx.golem_clients().await?;

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let selected_component_names = app_ctx
            .selected_component_names()
            .iter()
            .map(|cn| cn.as_str().parse())
            .collect::<Result<Vec<ComponentName>, _>>()
            .map_err(|err| anyhow!(err))?;

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;
        let current_deployment = environment.current_deployment_or_err()?;

        let mut components = Vec::with_capacity(selected_component_names.len());
        for component_name in &selected_component_names {
            match clients
                .component
                .get_deployment_component(
                    &environment.environment_id.0,
                    current_deployment.revision.into(),
                    &component_name.0,
                )
                .await
                .map_service_error_not_found_as_opt()?
            {
                Some(component) => {
                    components.push(component);
                }
                None => {
                    log_error(format!(
                        "Component {} is not deployed!",
                        component_name.0.log_color_highlight()
                    ));
                    bail!(NonSuccessfulExit)
                }
            }
        }
        Ok(components)
    }

    pub async fn must_select_components(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.opt_select_components(component_names, default)
            .await?
            .then_some(())
            .ok_or(anyhow!(HintError::NoApplicationManifestFound))
    }

    pub async fn opt_select_components(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<bool> {
        self.opt_select_components_internal(component_names, default, false)
            .await
    }

    pub async fn opt_select_components_allow_not_found(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<bool> {
        self.opt_select_components_internal(component_names, default, true)
            .await
    }

    // TODO: forbid matching the same component multiple times
    // Returns false if there is no app
    async fn opt_select_components_internal(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
        allow_not_found: bool,
    ) -> anyhow::Result<bool> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        let silent_selection = app_ctx.silent_init;
        let Some(app_ctx) = app_ctx.opt_mut()? else {
            return Ok(false);
        };

        if component_names.is_empty() {
            let _log_output = silent_selection.then(|| LogOutput::new(Output::TracingDebug));
            app_ctx.select_components(default)?
        } else {
            let fuzzy_search = FuzzySearch::new(
                app_ctx
                    .application()
                    .component_names()
                    .map(|cn| cn.as_str()),
            );

            let (found, not_found) =
                fuzzy_search.find_many(component_names.iter().map(|cn| cn.0.as_str()));

            if !not_found.is_empty() {
                if allow_not_found {
                    return Ok(false);
                }

                logln("");
                log_error(format!(
                    "The following requested component names were not found:\n{}",
                    not_found
                        .iter()
                        .map(|error| {
                            match error {
                                Error::Ambiguous {
                                    pattern,
                                    highlighted_options,
                                    ..
                                } => {
                                    format!(
                                        "  - {}, did you mean one of {}?",
                                        pattern.as_str().bold(),
                                        highlighted_options.iter().map(|cn| cn.bold()).join(", ")
                                    )
                                }
                                Error::NotFound { pattern } => {
                                    format!("  - {}", pattern.as_str().bold())
                                }
                            }
                        })
                        .join("\n")
                ));
                logln("");
                log_text_view(&AvailableComponentNamesHelp(
                    app_ctx.application().component_names().cloned().collect(),
                ));

                bail!(NonSuccessfulExit);
            }

            log_fuzzy_matches(&found);

            let _log_output = silent_selection.then(|| LogOutput::new(Output::TracingDebug));
            app_ctx.select_components(&ApplicationComponentSelectMode::Explicit(
                found.into_iter().map(|m| ComponentName(m.option)).collect(),
            ))?
        }
        Ok(true)
    }

    pub fn get_template(
        &self,
        requested_template_name: &str,
        dev_mode: bool,
    ) -> anyhow::Result<(Option<&Template>, &Template)> {
        let segments = requested_template_name.split("/").collect::<Vec<_>>();
        let (language, template_name): (String, Option<String>) = match segments.len() {
            1 => (segments[0].to_string(), None),
            2 => (segments[0].to_string(), {
                let template_name = segments[1].to_string();
                if template_name.is_empty() {
                    None
                } else {
                    Some(template_name)
                }
            }),
            _ => {
                log_error("Failed to parse template name");
                self.log_templates_help(None, None, self.ctx.dev_mode());
                bail!(NonSuccessfulExit);
            }
        };

        let language = match GuestLanguage::from_string(language) {
            Some(language) => language,
            None => {
                log_error("Failed to parse language part of the template!");
                self.log_templates_help(None, None, self.ctx.dev_mode());
                bail!(NonSuccessfulExit);
            }
        };
        let template_name = template_name
            .map(TemplateName::from)
            .unwrap_or_else(|| TemplateName::from("default"));

        let Some(lang_templates) = self.ctx.templates(dev_mode).get(&language) else {
            log_error(format!("No templates found for language: {language}").as_str());
            self.log_templates_help(None, None, self.ctx.dev_mode());
            bail!(NonSuccessfulExit);
        };

        let lang_templates = lang_templates
            .get(self.ctx.template_group())
            .ok_or_else(|| {
                anyhow!(
                    "No templates found for group: {}",
                    self.ctx.template_group().as_str().log_color_highlight()
                )
            })?;

        let Some(component_template) = lang_templates.components.get(&template_name) else {
            log_error(format!(
                "Template {} not found!",
                requested_template_name.log_color_highlight()
            ));
            self.log_templates_help(None, None, self.ctx.dev_mode());
            bail!(NonSuccessfulExit);
        };

        Ok((lang_templates.common.as_ref(), component_template))
    }

    pub fn log_languages_help(&self) {
        logln(format!("\n{}", "Available languages:".underline().bold(),));
        for language in GuestLanguage::iter() {
            logln(format!(
                "- {}: {}",
                language.name(),
                language.id().log_color_highlight()
            ));
        }
    }

    pub fn log_templates_help(
        &self,
        language_filter: Option<GuestLanguage>,
        template_filter: Option<&str>,
        dev_mode: bool,
    ) {
        if language_filter.is_none() && template_filter.is_none() {
            logln(format!(
                "\n{}",
                "Available languages and templates:".underline().bold(),
            ));
        } else {
            logln(format!("\n{}", "Matching templates:".underline().bold(),));
        }

        let templates = self
            .ctx
            .templates(dev_mode)
            .iter()
            .filter_map(|(language, templates)| {
                templates
                    .get(self.ctx.template_group())
                    .and_then(|templates| {
                        let matches_lang = language_filter
                            .map(|language_filter| language_filter == *language)
                            .unwrap_or(true);

                        if matches_lang {
                            let templates = templates
                                .components
                                .iter()
                                .filter(|(template_name, template)| {
                                    template_filter
                                        .map(|template_filter| {
                                            template_name
                                                .as_str()
                                                .to_lowercase()
                                                .contains(template_filter)
                                                || template
                                                    .description
                                                    .to_lowercase()
                                                    .contains(template_filter)
                                        })
                                        .unwrap_or(true)
                                })
                                .collect::<Vec<_>>();

                            (!templates.is_empty()).then_some(templates)
                        } else {
                            None
                        }
                    })
                    .map(|templates| (language, templates))
            })
            .collect::<Vec<_>>();

        for (language, templates) in templates {
            if let Some(language_filter) = language_filter {
                if language_filter != *language {
                    continue;
                }
            }

            logln(format!("- {}", language.to_string().bold()));
            for (template_name, template) in templates {
                if template_name.as_str() == "default" {
                    logln(format!(
                        "  - {}: {}",
                        language.id().bold(),
                        template.description,
                    ));
                } else {
                    logln(format!(
                        "  - {}/{}: {}",
                        language.id().bold(),
                        template.name.as_str().bold(),
                        template.description,
                    ));
                }
            }
        }
    }

    pub async fn diagnose(
        &self,
        component_names: Vec<ComponentName>,
        default_component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, default_component_select_mode)
            .await?;

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let selected_component_names = app_ctx
            .selected_component_names()
            .iter()
            .collect::<Vec<_>>();

        if selected_component_names.is_empty() {
            log_warn("The application has no components.");
        }

        for component_name in selected_component_names {
            log_action(
                "Diagnosing",
                format!(
                    "component {} for recommended tooling",
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = self.ctx.log_handler().nested_text_view_indent();

            diagnose(
                app_ctx.application().component(component_name).source(),
                None,
            );
        }

        Ok(())
    }

    async fn show_available_deployments(
        &self,
        environment: &ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<()> {
        logln("");
        logln("Available deployments:".log_color_help_group().to_string());
        logln("");

        let deployments = self
            .ctx
            .golem_clients()
            .await?
            .environment
            .list_deployments(&environment.environment_id.0, None)
            .await
            .map_service_error()?
            .values;
        self.ctx.log_handler().log_view(&deployments);

        Ok(())
    }

    async fn safe_show_available_deployments(&self, environment: &ResolvedEnvironmentIdentity) {
        if let Some(err) = self.show_available_deployments(environment).await.err() {
            log_error(format!("Failed to show available deployments: {}", err));
        }
    }
}
