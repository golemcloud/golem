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

use crate::clients::template::{TemplateClient, TemplateView};
use crate::model::{
    GolemError, GolemResult, PathBufOrStdin, RawTemplateId, TemplateIdOrName, TemplateName,
};

#[derive(Subcommand, Debug)]
#[command()]
pub enum TemplateSubcommand {
    #[command()]
    Add {
        #[arg(short, long)]
        template_name: TemplateName,

        #[arg(value_name = "template-file", value_hint = clap::ValueHint::FilePath)]
        template_file: PathBufOrStdin, // TODO: validate exists
    },

    #[command()]
    Update {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(value_name = "template-file", value_hint = clap::ValueHint::FilePath)]
        template_file: PathBufOrStdin, // TODO: validate exists
    },

    #[command()]
    List {
        #[arg(short, long)]
        template_name: Option<TemplateName>,
    },
}

#[async_trait]
pub trait TemplateHandler {
    async fn handle(&self, subcommand: TemplateSubcommand) -> Result<GolemResult, GolemError>;

    async fn resolve_id(&self, reference: TemplateIdOrName) -> Result<RawTemplateId, GolemError>;
}

pub struct TemplateHandlerLive<C: TemplateClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: TemplateClient + Send + Sync> TemplateHandler for TemplateHandlerLive<C> {
    async fn handle(&self, subcommand: TemplateSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            TemplateSubcommand::Add {
                template_name,
                template_file,
            } => {
                let template = self.client.add(template_name, template_file).await?;

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
            TemplateSubcommand::List { template_name } => {
                let templates = self.client.find(template_name).await?;

                Ok(GolemResult::Ok(Box::new(templates)))
            }
        }
    }

    async fn resolve_id(&self, reference: TemplateIdOrName) -> Result<RawTemplateId, GolemError> {
        match reference {
            TemplateIdOrName::Id(id) => Ok(id),
            TemplateIdOrName::Name(name) => {
                let templates = self.client.find(Some(name.clone())).await?;
                let templates: Vec<TemplateView> = templates
                    .into_iter()
                    .group_by(|c| c.template_id.clone())
                    .into_iter()
                    .map(|(_, group)| group.max_by_key(|c| c.template_version).unwrap())
                    .collect();

                if templates.len() > 1 {
                    let template_name = name.0;
                    let ids: Vec<String> = templates.into_iter().map(|c| c.template_id).collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple templates found for name {template_name}:
                        {}
                        Use explicit --template-id
                    ",
                        ids.join(", ")
                    )))
                } else {
                    match templates.first() {
                        None => {
                            let template_name = name.0;
                            Err(GolemError(format!("Can't find template ${template_name}")))
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
