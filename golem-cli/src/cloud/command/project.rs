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

use clap::Subcommand;

use crate::cloud::service::project::ProjectService;
use crate::model::{GolemError, GolemResult};

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

impl ProjectSubcommand {
    pub async fn handle(
        self,
        service: &(dyn ProjectService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ProjectSubcommand::Add {
                project_name,
                project_description,
            } => service.add(project_name, project_description).await,
            ProjectSubcommand::List { project_name } => service.list(project_name).await,
            ProjectSubcommand::GetDefault {} => service.get_default().await,
        }
    }
}
