// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::cloud::AccountId;
use crate::command::cloud::project::{ProjectActionsOrPolicyId, ProjectSubcommand};
use crate::command_handler::Handlers;
use crate::config::ProfileKind;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::HintError;
use crate::error::NonSuccessfulExit;
use crate::model::project::ProjectView;
use crate::model::text::fmt::{log_error, log_text_view};
use crate::model::text::help::ComponentNameHelp;
use crate::model::text::project::{
    ProjectCreatedView, ProjectGetView, ProjectGrantView, ProjectListView,
};
use crate::model::{ProjectName, ProjectNameAndId};
use anyhow::{anyhow, bail};
use golem_cloud_client::api::{ProjectClient, ProjectGrantClient};
use golem_cloud_client::model::{Project, ProjectDataRequest, ProjectGrantDataRequest};
use golem_wasm_rpc_stubgen::log::{logln, LogColorize};
use std::sync::Arc;

pub struct CloudProjectCommandHandler {
    ctx: Arc<Context>,
}

impl CloudProjectCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&mut self, subcommand: ProjectSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ProjectSubcommand::New {
                project_name,
                description,
            } => self.cmd_new(project_name, description).await,
            ProjectSubcommand::List { project_name } => self.cmd_list(project_name).await,
            ProjectSubcommand::GetDefault => self.cmd_get_default().await,
            ProjectSubcommand::Grant {
                project_name,
                recipient_account_id,
                project_actions_or_policy_id,
            } => {
                self.cmd_grant(
                    project_name,
                    recipient_account_id,
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
        }
    }

    async fn cmd_new(
        &mut self,
        project_name: ProjectName,
        description: Option<String>,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients_cloud().await?;
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

    async fn cmd_list(&mut self, project_name: Option<ProjectName>) -> anyhow::Result<()> {
        let projects = self
            .ctx
            .golem_clients_cloud()
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

    async fn cmd_get_default(&mut self) -> anyhow::Result<()> {
        let project = self
            .ctx
            .golem_clients_cloud()
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

    async fn opt_project_by_name(
        &self,
        account_id: Option<&AccountId>,
        project_name: &ProjectName,
    ) -> anyhow::Result<Option<Project>> {
        let mut projects = self
            .ctx
            .golem_clients_cloud()
            .await?
            .project
            .get_projects(Some(&project_name.0))
            .await
            .map_service_error()?;

        match account_id {
            Some(account_id) => {
                let project_idx = projects
                    .iter()
                    .position(|project| project.project_data.owner_account_id == account_id.0);
                match project_idx {
                    Some(project_idx) => Ok(Some(projects.swap_remove(project_idx))),
                    None => Ok(None),
                }
            }
            None => match projects.len() {
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
            },
        }
    }

    pub async fn project_by_name(
        &self,
        account_id: Option<&AccountId>,
        project_name: &ProjectName,
    ) -> anyhow::Result<Project> {
        match self.opt_project_by_name(account_id, project_name).await? {
            Some(project) => Ok(project),
            None => Err(project_not_found(account_id, project_name)),
        }
    }

    // TODO: special care might be needed for ordering app loading if
    //       project selection can be defined if app manifest too
    pub async fn opt_select_project(
        &self,
        account_id: Option<&AccountId>,
        project_name: Option<&ProjectName>,
    ) -> anyhow::Result<Option<ProjectNameAndId>> {
        match (self.ctx.profile_kind(), project_name) {
            (ProfileKind::Oss, Some(_)) => {
                log_error("Cannot use projects with OSS profile!");
                logln("");
                log_text_view(&ComponentNameHelp);
                logln("");
                bail!(HintError::ExpectedCloudProfile);
            }
            (ProfileKind::Oss, None) => Ok(None),
            (ProfileKind::Cloud, Some(project_name)) => {
                let project = self.project_by_name(account_id, project_name).await?;
                Ok(Some(ProjectNameAndId {
                    project_name: project.project_data.name.into(),
                    project_id: project.project_id.into(),
                }))
            }
            (ProfileKind::Cloud, None) => Ok(None),
        }
    }

    pub async fn select_project(
        &self,
        account_id: Option<&AccountId>,
        project_name: &ProjectName,
    ) -> anyhow::Result<ProjectNameAndId> {
        match self
            .opt_select_project(account_id, Some(project_name))
            .await?
        {
            Some(project) => Ok(project),
            None => Err(project_not_found(account_id, project_name)),
        }
    }

    pub async fn selected_project_or_default(
        &self,
        project: Option<ProjectNameAndId>,
    ) -> anyhow::Result<ProjectNameAndId> {
        match project {
            Some(project_name) => Ok(project_name),
            None => self
                .ctx
                .golem_clients_cloud()
                .await?
                .project
                .get_default_project()
                .await
                .map_service_error()
                .map(|project| ProjectNameAndId {
                    project_name: project.project_data.name.into(),
                    project_id: project.project_id.into(),
                }),
        }
    }

    async fn cmd_grant(
        &self,
        project_name: ProjectName,
        account_id: AccountId,
        actions_or_policy_id: ProjectActionsOrPolicyId,
    ) -> anyhow::Result<()> {
        let grant = self
            .ctx
            .golem_clients_cloud()
            .await?
            .project_grant
            .create_project_grant(
                &self.select_project(None, &project_name).await?.project_id.0,
                &ProjectGrantDataRequest {
                    grantee_account_id: account_id.0,
                    project_policy_id: actions_or_policy_id.policy_id.map(|id| id.0),
                    project_actions: actions_or_policy_id
                        .action
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| a.into())
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

fn project_not_found(account_id: Option<&AccountId>, project_name: &ProjectName) -> anyhow::Error {
    let formatted_account = account_id
        .map(|id| format!("{}/", id.0.log_color_highlight()))
        .unwrap_or_default();
    log_error(format!(
        "Project {}{} not found.",
        formatted_account,
        project_name.0.log_color_highlight()
    ));
    anyhow!(NonSuccessfulExit)
}
