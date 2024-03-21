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
use indoc::formatdoc;
use itertools::Itertools;
use uuid::Uuid;

use crate::clients::project::ProjectClient;
use crate::clients::template::{TemplateClient, TemplateView};
use crate::model::{
    GolemError, GolemResult, PathBufOrStdin, ProjectId, ProjectRef, RawTemplateId,
    TemplateIdOrName, TemplateName,
};

#[derive(Subcommand, Debug)]
#[command()]
pub enum TemplateSubcommand {
    /// Creates a new template with a given name by uploading the template WASM
    #[command()]
    Add {
        /// The newly created template's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Name of the newly created template
        #[arg(short, long)]
        template_name: TemplateName,

        /// The WASM file to be used as a Golem template
        #[arg(value_name = "template-file", value_hint = clap::ValueHint::FilePath)]
        template_file: PathBufOrStdin, // TODO: validate exists
    },

    /// Updates an existing template by uploading a new version of its WASM
    #[command()]
    Update {
        /// The template name or identifier to update
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// The WASM file to be used as as a new version of the Golem template
        #[arg(value_name = "template-file", value_hint = clap::ValueHint::FilePath)]
        template_file: PathBufOrStdin, // TODO: validate exists
    },

    /// Lists the existing templates
    #[command()]
    List {
        /// The project to list templates from
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Optionally look for only templates matching a given name
        #[arg(short, long)]
        template_name: Option<TemplateName>,
    },
}

#[async_trait]
pub trait TemplateHandler {
    async fn handle(&self, subcommand: TemplateSubcommand) -> Result<GolemResult, GolemError>;

    async fn resolve_id(&self, reference: TemplateIdOrName) -> Result<RawTemplateId, GolemError>;
}

pub struct TemplateHandlerLive<'p, C: TemplateClient + Send + Sync, P: ProjectClient + Sync + Send>
{
    pub client: C,
    pub projects: &'p P,
}

#[async_trait]
impl<'p, C: TemplateClient + Send + Sync, P: ProjectClient + Sync + Send> TemplateHandler
    for TemplateHandlerLive<'p, C, P>
{
    async fn handle(&self, subcommand: TemplateSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            TemplateSubcommand::Add {
                project_ref,
                template_name,
                template_file,
            } => {
                let project_id = self.projects.resolve_id(project_ref).await?;
                let template = self
                    .client
                    .add(project_id, template_name, template_file)
                    .await?;

                Ok(GolemResult::Ok(Box::new(template)))
            }
            TemplateSubcommand::Update {
                template_id_or_name,
                template_file,
            } => {
                let id = self.resolve_id(template_id_or_name).await?;
                let template = self.client.update(id, template_file).await?;

                Ok(GolemResult::Ok(Box::new(template)))
            }
            TemplateSubcommand::List {
                project_ref,
                template_name,
            } => {
                let project_id = self.projects.resolve_id(project_ref).await?;
                let templates = self.client.find(project_id, template_name).await?;

                Ok(GolemResult::Ok(Box::new(templates)))
            }
        }
    }

    async fn resolve_id(&self, reference: TemplateIdOrName) -> Result<RawTemplateId, GolemError> {
        match reference {
            TemplateIdOrName::Id(id) => Ok(id),
            TemplateIdOrName::Name(name, project_ref) => {
                let project_id = self.projects.resolve_id(project_ref).await?;
                let templates = self
                    .client
                    .find(project_id.clone(), Some(name.clone()))
                    .await?;
                let templates: Vec<TemplateView> = templates
                    .into_iter()
                    .group_by(|c| c.template_id.clone())
                    .into_iter()
                    .map(|(_, group)| group.max_by_key(|c| c.template_version).unwrap())
                    .collect();

                if templates.len() > 1 {
                    let project_str =
                        project_id.map_or("default".to_string(), |ProjectId(id)| id.to_string());
                    let template_name = name.0;
                    let ids: Vec<String> = templates.into_iter().map(|c| c.template_id).collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple templates found for name {template_name} in project {project_str}:
                        {}
                        Use explicit --template-id
                    ",
                        ids.join(", ")
                    )))
                } else {
                    match templates.first() {
                        None => {
                            let project_str = project_id
                                .map_or("default".to_string(), |ProjectId(id)| id.to_string());
                            let template_name = name.0;
                            Err(GolemError(format!(
                                "Can't find template ${template_name} in {project_str}"
                            )))
                        }
                        Some(template) => {
                            let parsed = Uuid::parse_str(&template.template_id);

                            match parsed {
                                Ok(id) => Ok(RawTemplateId(id)),
                                Err(err) => {
                                    Err(GolemError(format!("Failed to parse template id: {err}")))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
