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
use golem_client::model::Template;
use indoc::formatdoc;
use itertools::Itertools;

use crate::clients::template::TemplateClient;
use crate::model::template::TemplateView;
use crate::model::text::{TemplateAddView, TemplateUpdateView};
use crate::model::{
    GolemError, GolemResult, PathBufOrStdin, TemplateId, TemplateIdOrName, TemplateName,
};

#[derive(Subcommand, Debug)]
#[command()]
pub enum TemplateSubcommand {
    /// Creates a new template with a given name by uploading the template WASM
    #[command()]
    Add {
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
        /// Optionally look for only templates matching a given name
        #[arg(short, long)]
        template_name: Option<TemplateName>,
    },
}

#[async_trait]
pub trait TemplateHandler {
    async fn handle(&self, subcommand: TemplateSubcommand) -> Result<GolemResult, GolemError>;

    async fn resolve_id(&self, reference: TemplateIdOrName) -> Result<TemplateId, GolemError>;

    async fn get_metadata(
        &self,
        template_id: &TemplateId,
        version: u64,
    ) -> Result<Template, GolemError>;

    async fn get_latest_metadata(&self, template_id: &TemplateId) -> Result<Template, GolemError>;
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
                let view: TemplateView = template.into();

                Ok(GolemResult::Ok(Box::new(TemplateAddView(view))))
            }
            TemplateSubcommand::Update {
                template_id_or_name,
                template_file,
            } => {
                let id = self.resolve_id(template_id_or_name).await?;
                let template = self.client.update(id, template_file).await?;
                let view: TemplateView = template.into();

                Ok(GolemResult::Ok(Box::new(TemplateUpdateView(view))))
            }
            TemplateSubcommand::List { template_name } => {
                let templates = self.client.find(template_name).await?;
                let views: Vec<TemplateView> = templates.into_iter().map(|t| t.into()).collect();

                Ok(GolemResult::Ok(Box::new(views)))
            }
        }
    }

    async fn resolve_id(&self, reference: TemplateIdOrName) -> Result<TemplateId, GolemError> {
        match reference {
            TemplateIdOrName::Id(id) => Ok(id),
            TemplateIdOrName::Name(name) => {
                let templates = self.client.find(Some(name.clone())).await?;
                let templates: Vec<Template> = templates
                    .into_iter()
                    .group_by(|c| c.versioned_template_id.template_id)
                    .into_iter()
                    .map(|(_, group)| {
                        group
                            .max_by_key(|c| c.versioned_template_id.version)
                            .unwrap()
                    })
                    .collect();

                if templates.len() > 1 {
                    let template_name = name.0;
                    let ids: Vec<String> = templates
                        .into_iter()
                        .map(|c| c.versioned_template_id.template_id.to_string())
                        .collect();
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
                            Err(GolemError(format!("Can't find template {template_name}")))
                        }
                        Some(template) => {
                            Ok(TemplateId(template.versioned_template_id.template_id))
                        }
                    }
                }
            }
        }
    }

    async fn get_metadata(
        &self,
        template_id: &TemplateId,
        version: u64,
    ) -> Result<Template, GolemError> {
        self.client.get_metadata(template_id, version).await
    }

    async fn get_latest_metadata(&self, template_id: &TemplateId) -> Result<Template, GolemError> {
        self.client.get_latest_metadata(template_id).await
    }
}
