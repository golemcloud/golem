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

use crate::command::cloud::project::{ProjectActionsOrPolicyId, ProjectSubcommand};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{logln, LogColorize};
use crate::model::project::ProjectView;
use crate::model::text::fmt::{log_error, log_text_view};
use crate::model::text::project::{
    ProjectCreatedView, ProjectGetView, ProjectGrantView, ProjectListView,
};
use crate::model::ProjectId;
use crate::model::{ProjectName, ProjectRefAndId, ProjectReference};
use anyhow::{anyhow, bail};
use golem_client::api::{ProjectClient, ProjectGrantClient};
use golem_client::model::{Project, ProjectDataRequest, ProjectGrantDataRequest};
use std::sync::Arc;

pub mod plugin;
pub mod policy;

pub struct CloudProjectCommandHandler {
    ctx: Arc<Context>,
}

impl CloudProjectCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: ProjectSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ProjectSubcommand::New {
                project_name,
                description,
            } => self.cmd_new(project_name, description).await,
            ProjectSubcommand::List { project_name } => self.cmd_list(project_name).await,
            ProjectSubcommand::GetDefault => self.cmd_get_default().await,
            ProjectSubcommand::Grant {
                project_reference,
                recipient_email,
                project_actions_or_policy_id,
            } => {
                self.cmd_grant(
                    project_reference,
                    recipient_email,
                    project_actions_or_policy_id,
                )
                .await
            }
            ProjectSubcommand::Policy { subcommand } => {
                self.ctx
                    .cloud_project_policy_handler()
                    .handler_command(subcommand)
                    .await
            }
            ProjectSubcommand::Plugin { subcommand } => {
                self.ctx
                    .cloud_project_plugin_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }

    async fn cmd_new(
        &self,
        project_name: ProjectName,
        description: Option<String>,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        let project = clients
            .project
            .create_project(&ProjectDataRequest {
                name: project_name.0,
                owner_account_id: clients.account_id().0.to_string(),
                description: description.unwrap_or_default(),
            })
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_view(&ProjectCreatedView(ProjectView::from(project)));
        Ok(())
    }

    async fn cmd_list(&self, project_name: Option<ProjectName>) -> anyhow::Result<()> {
        let projects = self
            .ctx
            .golem_clients()
            .await?
            .project
            .get_projects(project_name.as_ref().map(|name| name.0.as_str()))
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_view(&ProjectListView::from(projects));
        Ok(())
    }

    async fn cmd_get_default(&self) -> anyhow::Result<()> {
        let project = self
            .ctx
            .golem_clients()
            .await?
            .project
            .get_default_project()
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_view(&ProjectGetView::from(project));
        Ok(())
    }

    async fn opt_project_by_reference(
        &self,
        project_reference: &ProjectReference,
    ) -> anyhow::Result<Option<Project>> {
        match project_reference {
            ProjectReference::JustName(project_name) => {
                let mut projects = self
                    .ctx
                    .golem_clients()
                    .await?
                    .project
                    .get_projects(Some(&project_name.0))
                    .await
                    .map_service_error()?;

                match projects.len() {
                    0 => Ok(None),
                    1 => Ok(Some(projects.pop().unwrap())),
                    _ => {
                        log_error(format!(
                            "Project name {} is ambiguous!",
                            project_name.0.log_color_highlight()
                        ));
                        logln("");
                        log_text_view(&ProjectListView::from(projects));
                        bail!(NonSuccessfulExit);
                    }
                }
            }
            ProjectReference::WithAccount {
                account_email,
                project_name,
            } => {
                let account = self
                    .ctx
                    .select_account_by_email_or_error(account_email)
                    .await?;

                let mut projects = self
                    .ctx
                    .golem_clients()
                    .await?
                    .project
                    .get_projects(Some(&project_name.0))
                    .await
                    .map_service_error()?;
                let project_idx = projects.iter().position(|project| {
                    project.project_data.owner_account_id == account.account_id.0
                });
                match project_idx {
                    Some(project_idx) => Ok(Some(projects.swap_remove(project_idx))),
                    None => Ok(None),
                }
            }
        }
    }

    pub async fn project_by_reference(
        &self,
        project_reference: &ProjectReference,
    ) -> anyhow::Result<Project> {
        match self.opt_project_by_reference(project_reference).await? {
            Some(project) => Ok(project),
            None => Err(project_not_found(project_reference)),
        }
    }

    pub async fn opt_select_project(
        &self,
        project_reference: Option<&ProjectReference>,
    ) -> anyhow::Result<Option<ProjectRefAndId>> {
        let project_reference = project_reference.or_else(|| self.ctx.profile_project());

        let ref_and_id = match project_reference {
            Some(project_reference) => {
                let project = self.project_by_reference(project_reference).await?;
                Some(ProjectRefAndId {
                    project_ref: project_reference.clone(),
                    project_id: project.project_id.into(),
                })
            }
            None => None,
        };

        Ok(ref_and_id)
    }

    pub async fn select_project(
        &self,
        project_reference: &ProjectReference,
    ) -> anyhow::Result<ProjectRefAndId> {
        match self.opt_select_project(Some(project_reference)).await? {
            Some(project) => Ok(project),
            None => Err(project_not_found(project_reference)),
        }
    }

    pub async fn selected_project_id_or_default(
        &self,
        project: Option<&ProjectRefAndId>,
    ) -> anyhow::Result<ProjectId> {
        // TODO: cache default project
        match project {
            Some(project) => Ok(project.project_id),
            None => self
                .ctx
                .golem_clients()
                .await?
                .project
                .get_default_project()
                .await
                .map_service_error()
                .map(|project| ProjectId(project.project_id)),
        }
    }

    async fn cmd_grant(
        &self,
        project_reference: ProjectReference,
        account_email: String,
        actions_or_policy_id: ProjectActionsOrPolicyId,
    ) -> anyhow::Result<()> {
        let grant = self
            .ctx
            .golem_clients()
            .await?
            .project_grant
            .create_project_grant(
                &self.select_project(&project_reference).await?.project_id.0,
                &ProjectGrantDataRequest {
                    grantee_account_id: None,
                    grantee_email: Some(account_email),
                    project_policy_id: actions_or_policy_id.policy_id.map(|id| id.0),
                    project_actions: actions_or_policy_id
                        .action
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                    project_policy_name: None,
                },
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&ProjectGrantView(grant));

        Ok(())
    }
}

fn project_not_found(project_reference: &ProjectReference) -> anyhow::Error {
    match project_reference {
        ProjectReference::JustName(project_name) => log_error(format!(
            "Project {} not found.",
            project_name.0.log_color_highlight()
        )),
        ProjectReference::WithAccount {
            account_email,
            project_name,
        } => log_error(format!(
            "Project {}/{} not found.",
            account_email.log_color_highlight(),
            project_name.0.log_color_highlight()
        )),
    };
    anyhow!(NonSuccessfulExit)
}
