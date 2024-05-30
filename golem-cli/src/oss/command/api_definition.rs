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

use crate::model::{
    ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult, PathBufOrStdin,
};
use crate::oss::model::OssContext;
use crate::service::api_definition::ApiDefinitionService;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiDefinitionSubcommand {
    /// Lists all api definitions
    #[command()]
    List {
        /// Api definition id to get all versions. Optional.
        #[arg(short, long)]
        id: Option<ApiDefinitionId>,
    },

    /// Creates an api definition
    ///
    /// Golem API definition file format expected
    #[command()]
    Add {
        /// The Golem API definition file
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists
    },

    /// Updates an api definition
    ///
    /// Golem API definition file format expected
    #[command()]
    Update {
        /// The Golem API definition file
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists
    },

    /// Import OpenAPI file as api definition
    #[command()]
    Import {
        /// The OpenAPI json or yaml file to be used as the api definition
        ///
        /// Json format expected unless file name ends up in `.yaml`
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists
    },

    /// Retrieves metadata about an existing api definition
    #[command()]
    Get {
        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Version of the api definition
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,
    },

    /// Deletes an existing api definition
    #[command()]
    Delete {
        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Version of the api definition
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,
    },
}

impl ApiDefinitionSubcommand {
    pub async fn handle(
        self,
        service: &(dyn ApiDefinitionService<ProjectContext = OssContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        let ctx = &OssContext::EMPTY;

        match self {
            ApiDefinitionSubcommand::Get { id, version } => service.get(id, version, ctx).await,
            ApiDefinitionSubcommand::Add { definition } => service.add(definition, ctx).await,
            ApiDefinitionSubcommand::Update { definition } => service.update(definition, ctx).await,
            ApiDefinitionSubcommand::Import { definition } => service.import(definition, ctx).await,
            ApiDefinitionSubcommand::List { id } => service.list(id, ctx).await,
            ApiDefinitionSubcommand::Delete { id, version } => {
                service.delete(id, version, ctx).await
            }
        }
    }
}
