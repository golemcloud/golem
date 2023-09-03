use std::path::PathBuf;
use async_trait::async_trait;
use clap::Subcommand;
use indoc::formatdoc;
use itertools::Itertools;
use uuid::Uuid;
use crate::clients::CloudAuthentication;
use crate::clients::component::{ComponentClient, ComponentView};
use crate::clients::project::ProjectClient;
use crate::model::{ComponentIdOrName, ComponentName, GolemError, GolemResult, ProjectId, ProjectRef, RawComponentId};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        component_name: ComponentName,

        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBuf, // TODO: validate exists
    },

    #[command()]
    Update {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBuf, // TODO: validate exists
    },

    #[command()]
    List {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        component_name: Option<ComponentName>,
    },
}

#[async_trait]
pub trait ComponentHandler {
    async fn handle(&self, token: &CloudAuthentication, subcommand: ComponentSubcommand) -> Result<GolemResult, GolemError>;

    async fn resolve_id(&self, reference: ComponentIdOrName, auth: &CloudAuthentication) -> Result<RawComponentId, GolemError>;
}


pub struct ComponentHandlerLive<'p, C: ComponentClient + Send + Sync, P: ProjectClient + Sync + Send> {
    pub client: C,
    pub projects: &'p P,
}

#[async_trait]
impl<'p, C: ComponentClient + Send + Sync, P: ProjectClient + Sync + Send> ComponentHandler for ComponentHandlerLive<'p, C, P> {
    async fn handle(&self, auth: &CloudAuthentication, subcommand: ComponentSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ComponentSubcommand::Add { project_ref, component_name, component_file } => {
                let project_id = self.projects.resolve_id(project_ref, auth).await?;
                let component = self.client.add(project_id, component_name, component_file, auth).await?;

                Ok(GolemResult::Ok(Box::new(component)))
            }
            ComponentSubcommand::Update { component_id_or_name, component_file } => {
                let id = self.resolve_id(component_id_or_name, auth).await?;
                let component = self.client.update(id, component_file, auth).await?;

                Ok(GolemResult::Ok(Box::new(component)))
            }
            ComponentSubcommand::List { project_ref, component_name } => {
                let project_id = self.projects.resolve_id(project_ref, auth).await?;
                let components = self.client.find(project_id, component_name, auth).await?;

                Ok(GolemResult::Ok(Box::new(components)))
            }
        }
    }

    async fn resolve_id(&self, reference: ComponentIdOrName, auth: &CloudAuthentication) -> Result<RawComponentId, GolemError> {
        match reference {
            ComponentIdOrName::Id(id) => Ok(id),
            ComponentIdOrName::Name(name, project_ref) => {
                let project_id = self.projects.resolve_id(project_ref, auth).await?;
                let components = self.client.find(project_id.clone(), Some(name.clone()), auth).await?;
                let components: Vec<ComponentView> =
                    components
                        .into_iter()
                        .group_by(|c| c.component_id.clone())
                        .into_iter()
                        .map(|(_, group)| group.max_by_key(|c| c.component_version).unwrap())
                        .collect();

                if components.len() > 1 {
                    let project_str = project_id.map_or("default".to_string(), |ProjectId(id)| id.to_string());
                    let component_name = name.0;
                    let ids: Vec<String> = components.into_iter().map(|c| c.component_id).collect();
                    Err(GolemError(formatdoc!("
                        Multiple components found for name {component_name} in project {project_str}:
                        {}
                        Use explicit --component-id
                    ",
                    ids.join(", "))))
                } else {
                    match components.first() {
                        None => {
                            let project_str = project_id.map_or("default".to_string(), |ProjectId(id)| id.to_string());
                            let component_name = name.0;
                            Err(GolemError(format!("Can't find component ${component_name} in {project_str}")))
                        }
                        Some(component) => {
                            let parsed = Uuid::parse_str(&component.component_id);

                            match parsed {
                                Ok(id) => Ok(RawComponentId(id)),
                                Err(err) => Err(GolemError(format!("Failed to parse component id: {err}"))),
                            }
                        }
                    }
                }
            }
        }
    }
}