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

use crate::cloud::model::ProjectRef;
use crate::cloud::service::domain::DomainService;
use clap::Subcommand;

use crate::model::{GolemError, GolemResult};

#[derive(Subcommand, Debug)]
#[command()]
pub enum DomainSubcommand {
    #[command()]
    Get {
        #[command(flatten)]
        project_ref: ProjectRef,
    },
    #[command()]
    Add {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long, value_hint = clap::ValueHint::Other)]
        domain_name: String,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(value_name = "domain-name", value_hint = clap::ValueHint::Other)]
        domain_name: String,
    },
}

impl DomainSubcommand {
    pub async fn handle(
        self,
        service: &(dyn DomainService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            DomainSubcommand::Get { project_ref } => service.get(project_ref).await,
            DomainSubcommand::Add {
                project_ref,
                domain_name,
            } => service.add(project_ref, domain_name).await,
            DomainSubcommand::Delete {
                project_ref,
                domain_name,
            } => service.delete(project_ref, domain_name).await,
        }
    }
}
