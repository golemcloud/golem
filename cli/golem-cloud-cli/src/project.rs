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

use async_trait::async_trait;
use clap::Subcommand;

use crate::clients::project::ProjectClient;
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError, GolemResult};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProjectSubcommand {
    /// Add a new project
    #[command()]
    Add {
        /// The new project's name
        #[arg(short, long)]
        project_name: String,

        /// The new project's description
        #[arg(short = 't', long)]
        project_description: Option<String>,
    },

    /// Lists existing projects
    #[command()]
    List {
        /// Optionally filter projects by name
        #[arg(short, long)]
        project_name: Option<String>,
    },

    /// Gets the default project which is used when no explicit project is specified
    #[command()]
    GetDefault {},
}

#[async_trait]
pub trait ProjectHandler {
    async fn handle(
        &self,
        token: &CloudAuthentication,
        subcommand: ProjectSubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ProjectHandlerLive<'c, C: ProjectClient + Send + Sync> {
    pub client: &'c C,
}

#[async_trait]
impl<'c, C: ProjectClient + Send + Sync> ProjectHandler for ProjectHandlerLive<'c, C> {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        subcommand: ProjectSubcommand,
    ) -> Result<GolemResult, GolemError> {
        match subcommand {
            ProjectSubcommand::Add {
                project_name,
                project_description,
            } => {
                let project = self
                    .client
                    .create(
                        AccountId {
                            id: auth.0.data.account_id.clone(),
                        },
                        project_name,
                        project_description,
                    )
                    .await?;

                Ok(GolemResult::Ok(Box::new(project)))
            }
            ProjectSubcommand::List { project_name } => {
                let projects = self.client.find(project_name).await?;

                Ok(GolemResult::Ok(Box::new(projects)))
            }
            ProjectSubcommand::GetDefault {} => {
                let project = self.client.find_default().await?;

                Ok(GolemResult::Ok(Box::new(project)))
            }
        }
    }
}
