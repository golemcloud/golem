use async_trait::async_trait;
use clap::Subcommand;
use indoc::formatdoc;
use itertools::Itertools;
use uuid::Uuid;

use crate::clients::project::ProjectClient;
use crate::clients::template::{TemplateClient, TemplateView};
use crate::clients::CloudAuthentication;
use crate::model::{
    GolemError, GolemResult, PathBufOrStdin, ProjectId, ProjectRef, RawTemplateId,
    TemplateIdOrName, TemplateName,
};

#[derive(Subcommand, Debug)]
#[command()]
pub enum TemplateSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        project_ref: ProjectRef,

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
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        template_name: Option<TemplateName>,
    },
}

#[async_trait]
pub trait TemplateHandler {
    async fn handle(
        &self,
        token: &CloudAuthentication,
        subcommand: TemplateSubcommand,
    ) -> Result<GolemResult, GolemError>;

    async fn resolve_id(
        &self,
        reference: TemplateIdOrName,
        auth: &CloudAuthentication,
    ) -> Result<RawTemplateId, GolemError>;
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
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        subcommand: TemplateSubcommand,
    ) -> Result<GolemResult, GolemError> {
        match subcommand {
            TemplateSubcommand::Add {
                project_ref,
                template_name,
                template_file,
            } => {
                let project_id = self.projects.resolve_id(project_ref, auth).await?;
                let template = self
                    .client
                    .add(project_id, template_name, template_file, auth)
                    .await?;

                Ok(GolemResult::Ok(Box::new(template)))
            }
            TemplateSubcommand::Update {
                template_id_or_name,
                template_file,
            } => {
                let id = self.resolve_id(template_id_or_name, auth).await?;
                let template = self.client.update(id, template_file, auth).await?;

                Ok(GolemResult::Ok(Box::new(template)))
            }
            TemplateSubcommand::List {
                project_ref,
                template_name,
            } => {
                let project_id = self.projects.resolve_id(project_ref, auth).await?;
                let templates = self.client.find(project_id, template_name, auth).await?;

                Ok(GolemResult::Ok(Box::new(templates)))
            }
        }
    }

    async fn resolve_id(
        &self,
        reference: TemplateIdOrName,
        auth: &CloudAuthentication,
    ) -> Result<RawTemplateId, GolemError> {
        match reference {
            TemplateIdOrName::Id(id) => Ok(id),
            TemplateIdOrName::Name(name, project_ref) => {
                let project_id = self.projects.resolve_id(project_ref, auth).await?;
                let templates = self
                    .client
                    .find(project_id.clone(), Some(name.clone()), auth)
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
