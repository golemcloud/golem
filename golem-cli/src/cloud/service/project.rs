// Copyright 2024 Golem Cloud
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

use crate::cloud::clients::project::ProjectClient;
use crate::cloud::model::{AccountId, ProjectId, ProjectRef};
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;
use golem_cloud_client::model::Project;
use indoc::formatdoc;

#[async_trait]
pub trait ProjectService {
    async fn add(
        &self,
        project_name: String,
        project_description: Option<String>,
    ) -> Result<GolemResult, GolemError>;
    async fn list(&self, project_name: Option<String>) -> Result<GolemResult, GolemError>;
    async fn get_default(&self) -> Result<GolemResult, GolemError>;

    async fn find_default(&self) -> Result<Project, GolemError>;
    async fn resolve_id(&self, project_ref: ProjectRef) -> Result<Option<ProjectId>, GolemError>;

    async fn resolve_id_or_default(
        &self,
        project_ref: ProjectRef,
    ) -> Result<ProjectId, GolemError> {
        match self.resolve_id(project_ref).await? {
            None => Ok(ProjectId(self.find_default().await?.project_id)),
            Some(project_id) => Ok(project_id),
        }
    }

    async fn resolve_id_or_default_opt(
        &self,
        project_ref: Option<ProjectRef>,
    ) -> Result<Option<ProjectId>, GolemError> {
        match project_ref {
            None => Ok(None),
            Some(project_ref) => Ok(Some(self.resolve_id_or_default(project_ref).await?)),
        }
    }
}

pub struct ProjectServiceLive {
    pub account_id: AccountId,
    pub client: Box<dyn ProjectClient + Send + Sync>,
}

#[async_trait]
impl ProjectService for ProjectServiceLive {
    async fn add(
        &self,
        project_name: String,
        project_description: Option<String>,
    ) -> Result<GolemResult, GolemError> {
        let project = self
            .client
            .create(&self.account_id, project_name, project_description)
            .await?;

        Ok(GolemResult::Ok(Box::new(project)))
    }

    async fn list(&self, project_name: Option<String>) -> Result<GolemResult, GolemError> {
        let projects = self.client.find(project_name).await?;

        Ok(GolemResult::Ok(Box::new(projects)))
    }

    async fn get_default(&self) -> Result<GolemResult, GolemError> {
        let project = self.find_default().await?;

        Ok(GolemResult::Ok(Box::new(project)))
    }

    async fn find_default(&self) -> Result<Project, GolemError> {
        self.client.find_default().await
    }

    async fn resolve_id(&self, project_ref: ProjectRef) -> Result<Option<ProjectId>, GolemError> {
        match project_ref {
            ProjectRef::Id(id) => Ok(Some(id)),
            ProjectRef::Name(name) => {
                let projects = self.client.find(Some(name.clone())).await?;

                if projects.len() > 1 {
                    let projects: Vec<String> =
                        projects.iter().map(|p| p.project_id.to_string()).collect();
                    Err(GolemError(formatdoc!(
                        "
                            Multiple projects found for name {name}:
                            {}
                            Use explicit --project-id or set target project as default.
                        ",
                        projects.join(", ")
                    )))
                } else {
                    match projects.first() {
                        None => Err(GolemError(format!("Can't find project with name {name}"))),
                        Some(project) => Ok(Some(ProjectId(project.project_id))),
                    }
                }
            }
            ProjectRef::Default => Ok(None),
        }
    }
}
