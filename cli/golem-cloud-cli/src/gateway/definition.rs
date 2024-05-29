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

use std::fs::File;
use std::io;
use std::io::{BufReader, Read};

use async_trait::async_trait;
use clap::Subcommand;
use golem_cloud_worker_client::model::HttpApiDefinition;

use crate::clients::gateway::definition::DefinitionClient;
use crate::clients::project::ProjectClient;
use crate::model::{Format, GolemError, GolemResult, PathBufOrStdin, ProjectRef};

#[derive(Subcommand, Debug)]
#[command()]
pub enum DefinitionSubcommand {
    #[command()]
    Get {
        #[command(flatten)]
        project_ref: ProjectRef,
        #[arg(value_name = "api-definition-id", value_hint = clap::ValueHint::Other)]
        definition_id: Option<String>,
    },
    #[command()]
    Update {
        #[command(flatten)]
        project_ref: ProjectRef,
        #[arg(value_name = "definition-file", value_hint = clap::ValueHint::FilePath)]
        definition_file: Option<PathBufOrStdin>,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        project_ref: ProjectRef,
        #[arg(value_name = "api-definition-id", value_hint = clap::ValueHint::Other)]
        definition_id: String,
        #[arg(value_name = "version", value_hint = clap::ValueHint::Other)]
        version: String,
    },
}

#[async_trait]
pub trait DefinitionHandler {
    async fn handle(
        &self,
        format: Format,
        command: DefinitionSubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct DefinitionHandlerLive<
    'p,
    C: DefinitionClient + Sync + Send,
    P: ProjectClient + Sync + Send,
> {
    pub client: C,
    pub projects: &'p P,
}

fn read_definition<R: Read>(
    format: Format,
    r: R,
    source: &str,
) -> Result<HttpApiDefinition, GolemError> {
    let api_definition: HttpApiDefinition = match format {
        Format::Json => serde_json::from_reader(r).map_err(|e| {
            GolemError(format!(
                "Failed to parse HttpApiDefinition from {source} as json: ${e}"
            ))
        })?,
        Format::Yaml => serde_yaml::from_reader(r).map_err(|e| {
            GolemError(format!(
                "Failed to parse HttpApiDefinition from {source} as yaml: ${e}"
            ))
        })?,
    };

    Ok(api_definition)
}

#[async_trait]
impl<'p, C: DefinitionClient + Sync + Send, P: ProjectClient + Sync + Send> DefinitionHandler
    for DefinitionHandlerLive<'p, C, P>
{
    async fn handle(
        &self,
        format: Format,
        command: DefinitionSubcommand,
    ) -> Result<GolemResult, GolemError> {
        match command {
            DefinitionSubcommand::Get {
                project_ref,
                definition_id,
            } => {
                let project_id = self.projects.resolve_id_or_default(project_ref).await?;

                let res = self
                    .client
                    .get(project_id, definition_id.as_deref())
                    .await?;

                Ok(GolemResult::Ok(Box::new(res)))
            }
            DefinitionSubcommand::Update {
                project_ref,
                definition_file,
            } => {
                let project_id = self.projects.resolve_id_or_default(project_ref).await?;

                let definition = match definition_file.unwrap_or(PathBufOrStdin::Stdin) {
                    PathBufOrStdin::Path(path) => {
                        let file = File::open(&path).map_err(|e| {
                            GolemError(format!("Failed to open file {path:?}: {e}"))
                        })?;

                        let reader = BufReader::new(file);

                        read_definition(format, reader, &format!("file `{path:?}`"))?
                    }
                    PathBufOrStdin::Stdin => read_definition(format, io::stdin(), "stdin")?,
                };

                let res = self.client.update(project_id, definition).await?;

                Ok(GolemResult::Ok(Box::new(res)))
            }
            DefinitionSubcommand::Delete {
                project_ref,
                definition_id,
                version,
            } => {
                let project_id = self.projects.resolve_id_or_default(project_ref).await?;
                let res = self
                    .client
                    .delete(project_id, &definition_id, version.as_str())
                    .await?;
                Ok(GolemResult::Ok(Box::new(res)))
            }
        }
    }
}
