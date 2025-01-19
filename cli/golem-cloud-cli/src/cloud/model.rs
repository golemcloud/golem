pub mod text;
pub mod to_cli;
pub mod to_cloud;
pub mod to_oss;

use async_trait::async_trait;
use clap::{ArgMatches, Error, FromArgMatches};
use derive_more::{Display, FromStr, Into};
use golem_cli::command::{ComponentRefSplit, ComponentRefsSplit};
use golem_cli::model::plugin_manifest::{FromPluginManifest, PluginManifest};
use golem_cli::model::{ComponentIdResolver, ComponentName, GolemError, PluginScopeArgs};
use golem_client::model::PluginTypeSpecificDefinition;
use golem_cloud_client::model::{
    PluginDefinitionCloudPluginOwnerCloudPluginScope, PluginDefinitionWithoutOwnerCloudPluginScope,
    Project, ProjectType,
};
use golem_cloud_client::{CloudPluginScope, ProjectPluginScope};
use golem_common::model::plugin::ComponentPluginScope;
use golem_common::model::{Empty, ProjectId};
use golem_common::uri::cloud::uri::{ComponentUri, ProjectUri, ToOssUri};
use golem_common::uri::cloud::url::ProjectUrl;
use golem_common::uri::cloud::urn::ProjectUrn;
use serde::{Deserialize, Serialize};
use std::fmt::Formatter;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Into)]
pub struct TokenId(pub Uuid);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProjectRef {
    pub uri: Option<ProjectUri>,
    pub explicit_name: bool,
}

impl FromArgMatches for ProjectRef {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ProjectRefArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: ProjectRefArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = ProjectRefArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for ProjectRef {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ProjectRefArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ProjectRefArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct ProjectRefArgs {
    #[arg(short = 'P', long, conflicts_with = "project_name")]
    pub project: Option<ProjectUri>,

    #[arg(short = 'p', long, conflicts_with = "project")]
    pub project_name: Option<String>,
}

impl From<&ProjectRefArgs> for ProjectRef {
    fn from(value: &ProjectRefArgs) -> ProjectRef {
        if let Some(uri) = &value.project {
            ProjectRef {
                uri: Some(uri.clone()),
                explicit_name: false,
            }
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef {
                uri: Some(ProjectUri::URL(ProjectUrl {
                    name,
                    account: None,
                })),
                explicit_name: true,
            }
        } else {
            ProjectRef {
                uri: None,
                explicit_name: false,
            }
        }
    }
}

impl From<&ProjectRef> for ProjectRefArgs {
    fn from(value: &ProjectRef) -> Self {
        match &value.uri {
            None => ProjectRefArgs {
                project: None,
                project_name: None,
            },
            Some(ProjectUri::URN(urn)) => ProjectRefArgs {
                project: Some(ProjectUri::URN(urn.clone())),
                project_name: None,
            },
            Some(ProjectUri::URL(url)) => {
                if value.explicit_name {
                    ProjectRefArgs {
                        project: None,
                        project_name: Some(url.name.to_string()),
                    }
                } else {
                    ProjectRefArgs {
                        project: Some(ProjectUri::URL(url.clone())),
                        project_name: None,
                    }
                }
            }
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct CloudPluginScopeArgs {
    /// Global scope (plugin available for all components)
    #[arg(long, conflicts_with_all=["component", "component_name", "project_id"])]
    global: bool,

    /// Component scope given by a component URN or URL (plugin only available for this component)
    #[arg(long, short = 'C', value_name = "URI", conflicts_with_all=["global", "component_name", "project_id"])]
    component: Option<ComponentUri>,

    /// Component scope given by the component's name (plugin only available for this component)
    #[arg(long, short = 'c', conflicts_with_all=["global", "component"], requires="project_id")]
    component_name: Option<String>,

    /// Plugin ID; Required when component name is used. Without a given component, it defines a project scope.
    #[arg(short = 'p', long, conflicts_with_all = ["global", "component"])]
    project_id: Option<Uuid>, // NOTE: this should be ProjectUri but we have no way currently to access a ProjectIdResolver in the type class implementation (see https://github.com/golemcloud/golem-cloud/issues/1573)
}

#[async_trait]
impl PluginScopeArgs for CloudPluginScopeArgs {
    type PluginScope = CloudPluginScope;
    type ComponentRef = CloudComponentUriOrName;

    async fn into(
        self,
        resolver: impl ComponentIdResolver<Self::ComponentRef> + Send,
    ) -> Result<Option<Self::PluginScope>, GolemError> {
        if self.global {
            Ok(Some(CloudPluginScope::Global(Empty {})))
        } else if let Some(uri) = self.component {
            let component_id = resolver.resolve(CloudComponentUriOrName::Uri(uri)).await?;
            Ok(Some(CloudPluginScope::Component(ComponentPluginScope {
                component_id,
            })))
        } else if let (Some(name), Some(project_id)) = (self.component_name, self.project_id) {
            let component_id = resolver
                .resolve(CloudComponentUriOrName::Name(
                    ComponentName(name),
                    ProjectRef {
                        uri: Some(ProjectUri::URN(ProjectUrn {
                            id: ProjectId(project_id),
                        })),
                        explicit_name: false,
                    },
                ))
                .await?;
            Ok(Some(CloudPluginScope::Component(ComponentPluginScope {
                component_id,
            })))
        } else if let Some(project_id) = self.project_id {
            Ok(Some(CloudPluginScope::Project(ProjectPluginScope {
                project_id: ProjectId(project_id),
            })))
        } else {
            Ok(None)
        }
    }
}

impl FromArgMatches for CloudComponentUriOrName {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        CloudComponentUriOrNameArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: CloudComponentUriOrNameArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = CloudComponentUriOrNameArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for CloudComponentUriOrName {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        CloudComponentUriOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        CloudComponentUriOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct CloudComponentUriOrNamesArgs {
    /// Component URI. Either URN or URL.
    #[arg(
        short = 'C',
        long,
        value_name = "URI",
        conflicts_with_all = vec!["component_name"],
    )]
    component: Option<ComponentUri>,

    /// Name of the component(s). When used with application manifest then multiple ones can be defined.
    #[arg(short, long)]
    component_name: Vec<String>,

    #[arg(
        short = 'P',
        long,
        conflicts_with = "project_name",
        conflicts_with = "component"
    )]
    project: Option<ProjectUri>,

    #[arg(
        short = 'p',
        long,
        conflicts_with = "project",
        conflicts_with = "component"
    )]
    project_name: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
struct CloudComponentUriOrNameArgs {
    #[arg(short = 'C', long, conflicts_with = "component_name", required = true)]
    component: Option<ComponentUri>,

    #[arg(short = 'c', long, conflicts_with = "component", required = true)]
    component_name: Option<String>,

    #[arg(
        short = 'P',
        long,
        conflicts_with = "project_name",
        conflicts_with = "component"
    )]
    project: Option<ProjectUri>,

    #[arg(
        short = 'p',
        long,
        conflicts_with = "project",
        conflicts_with = "component"
    )]
    project_name: Option<String>,
}

impl From<&CloudComponentUriOrNameArgs> for CloudComponentUriOrName {
    fn from(value: &CloudComponentUriOrNameArgs) -> CloudComponentUriOrName {
        let pr = if let Some(uri) = value.project.clone() {
            ProjectRef {
                uri: Some(uri),
                explicit_name: false,
            }
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef {
                uri: Some(ProjectUri::URL(ProjectUrl {
                    name,
                    account: None,
                })),
                explicit_name: true,
            }
        } else {
            ProjectRef {
                uri: None,
                explicit_name: false,
            }
        };

        if let Some(uri) = value.component.clone() {
            CloudComponentUriOrName::Uri(uri)
        } else {
            CloudComponentUriOrName::Name(
                ComponentName(value.component_name.as_ref().unwrap().to_string()),
                pr,
            )
        }
    }
}

impl From<&CloudComponentUriOrNamesArgs> for CloudComponentUriOrNames {
    fn from(value: &CloudComponentUriOrNamesArgs) -> CloudComponentUriOrNames {
        let pr = if let Some(uri) = value.project.clone() {
            ProjectRef {
                uri: Some(uri),
                explicit_name: false,
            }
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef {
                uri: Some(ProjectUri::URL(ProjectUrl {
                    name,
                    account: None,
                })),
                explicit_name: true,
            }
        } else {
            ProjectRef {
                uri: None,
                explicit_name: false,
            }
        };

        if let Some(uri) = value.component.clone() {
            CloudComponentUriOrNames::Uri(uri)
        } else {
            CloudComponentUriOrNames::Names(
                value
                    .component_name
                    .iter()
                    .map(|n| ComponentName(n.clone()))
                    .collect(),
                pr,
            )
        }
    }
}

impl From<&CloudComponentUriOrName> for CloudComponentUriOrNameArgs {
    fn from(value: &CloudComponentUriOrName) -> CloudComponentUriOrNameArgs {
        match value {
            CloudComponentUriOrName::Uri(uri) => CloudComponentUriOrNameArgs {
                component: Some(uri.clone()),
                component_name: None,
                project: None,
                project_name: None,
            },
            CloudComponentUriOrName::Name(ComponentName(name), pr) => {
                let ProjectRefArgs {
                    project,
                    project_name,
                } = ProjectRefArgs::from(pr);

                CloudComponentUriOrNameArgs {
                    component: None,
                    component_name: Some(name.clone()),
                    project,
                    project_name,
                }
            }
        }
    }
}

impl From<&CloudComponentUriOrNames> for CloudComponentUriOrNamesArgs {
    fn from(value: &CloudComponentUriOrNames) -> CloudComponentUriOrNamesArgs {
        match value {
            CloudComponentUriOrNames::Uri(uri) => CloudComponentUriOrNamesArgs {
                component: Some(uri.clone()),
                component_name: vec![],
                project: None,
                project_name: None,
            },
            CloudComponentUriOrNames::Names(names, pr) => {
                let ProjectRefArgs {
                    project,
                    project_name,
                } = ProjectRefArgs::from(pr);

                CloudComponentUriOrNamesArgs {
                    component: None,
                    component_name: names.iter().map(|n| n.0.clone()).collect(),
                    project,
                    project_name,
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CloudComponentUriOrName {
    Uri(ComponentUri),
    Name(ComponentName, ProjectRef),
}

impl ComponentRefSplit<ProjectRef> for CloudComponentUriOrName {
    fn split(
        self,
    ) -> (
        golem_common::uri::oss::uri::ComponentUri,
        Option<ProjectRef>,
    ) {
        match self {
            CloudComponentUriOrName::Uri(uri) => {
                let (uri, p) = uri.to_oss_uri();

                let p = ProjectRef {
                    uri: p.map(ProjectUri::URL),
                    explicit_name: false,
                };

                (uri, Some(p))
            }
            CloudComponentUriOrName::Name(name, p) => {
                let uri = golem_common::uri::oss::uri::ComponentUri::URL(
                    golem_common::uri::oss::url::ComponentUrl { name: name.0 },
                );

                (uri, Some(p))
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CloudComponentUriOrNames {
    Uri(ComponentUri),
    Names(Vec<ComponentName>, ProjectRef),
}

impl ComponentRefsSplit<ProjectRef> for CloudComponentUriOrNames {
    fn split(
        self,
    ) -> Option<(
        Vec<golem_common::uri::oss::uri::ComponentUri>,
        Option<ProjectRef>,
    )> {
        match self {
            CloudComponentUriOrNames::Uri(uri) => {
                let (uri, p) = uri.to_oss_uri();

                let p = ProjectRef {
                    uri: p.map(ProjectUri::URL),
                    explicit_name: false,
                };

                Some((vec![uri], Some(p)))
            }
            CloudComponentUriOrNames::Names(names, p) => {
                let p = Some(p);
                Some((
                    names
                        .into_iter()
                        .map(|n| {
                            golem_common::uri::oss::uri::ComponentUri::URL(
                                golem_common::uri::oss::url::ComponentUrl { name: n.0 },
                            )
                        })
                        .collect(),
                    p,
                ))
            }
        }
    }
}

impl FromArgMatches for CloudComponentUriOrNames {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        CloudComponentUriOrNamesArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: CloudComponentUriOrNamesArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = CloudComponentUriOrNamesArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for CloudComponentUriOrNames {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        CloudComponentUriOrNamesArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        CloudComponentUriOrNamesArgs::augment_args_for_update(cmd)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ProjectPolicyId(pub Uuid);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub project_urn: ProjectUrn,
    pub name: String,
    pub owner_account_id: String,
    pub description: String,
    pub default_environment_id: String,
    pub project_type: ProjectType,
}

impl From<Project> for ProjectView {
    fn from(value: Project) -> Self {
        Self {
            project_urn: ProjectUrn {
                id: ProjectId(value.project_id),
            },
            name: value.project_data.name.to_string(),
            owner_account_id: value.project_data.owner_account_id.to_string(),
            description: value.project_data.description.to_string(),
            default_environment_id: value.project_data.default_environment_id.to_string(),
            project_type: value.project_data.project_type.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginDefinition(pub PluginDefinitionCloudPluginOwnerCloudPluginScope);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginDefinitionWithoutOwner(pub PluginDefinitionWithoutOwnerCloudPluginScope);

impl FromPluginManifest for PluginDefinitionWithoutOwner {
    type PluginScope = CloudPluginScope;

    fn from_plugin_manifest(
        manifest: PluginManifest,
        scope: Self::PluginScope,
        specs: PluginTypeSpecificDefinition,
        icon: Vec<u8>,
    ) -> Self {
        PluginDefinitionWithoutOwner(PluginDefinitionWithoutOwnerCloudPluginScope {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            icon,
            homepage: manifest.homepage,
            specs,
            scope,
        })
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize)]
pub enum Role {
    Admin,
    MarketingAdmin,
    ViewProject,
    DeleteProject,
    CreateProject,
    InstanceServer,
    UpdateProject,
    ViewPlugin,
    CreatePlugin,
    DeletePlugin,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Admin => "Admin",
            Role::MarketingAdmin => "MarketingAdmin",
            Role::ViewProject => "ViewProject",
            Role::DeleteProject => "DeleteProject",
            Role::CreateProject => "CreateProject",
            Role::InstanceServer => "InstanceServer",
            Role::UpdateProject => "UpdateProject",
            Role::ViewPlugin => "ViewPlugin",
            Role::CreatePlugin => "CreatePlugin",
            Role::DeletePlugin => "DeletePlugin",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            "ViewProject" => Ok(Role::ViewProject),
            "DeleteProject" => Ok(Role::DeleteProject),
            "CreateProject" => Ok(Role::CreateProject),
            "InstanceServer" => Ok(Role::InstanceServer),
            "UpdateProject" => Ok(Role::UpdateProject),
            "ViewPlugin" => Ok(Role::ViewPlugin),
            "CreatePlugin" => Ok(Role::CreatePlugin),
            _ => {
                let all = Role::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown role: {s}. Expected one of {all}"))
            }
        }
    }
}

impl From<Role> for golem_cloud_client::model::Role {
    fn from(value: Role) -> Self {
        match value {
            Role::Admin => golem_cloud_client::model::Role::Admin,
            Role::MarketingAdmin => golem_cloud_client::model::Role::MarketingAdmin,
            Role::ViewProject => golem_cloud_client::model::Role::ViewProject,
            Role::DeleteProject => golem_cloud_client::model::Role::DeleteProject,
            Role::CreateProject => golem_cloud_client::model::Role::CreateProject,
            Role::InstanceServer => golem_cloud_client::model::Role::InstanceServer,
            Role::UpdateProject => golem_cloud_client::model::Role::UpdateProject,
            Role::ViewPlugin => golem_cloud_client::model::Role::ViewPlugin,
            Role::CreatePlugin => golem_cloud_client::model::Role::CreatePlugin,
            Role::DeletePlugin => golem_cloud_client::model::Role::DeletePlugin,
        }
    }
}

impl From<golem_cloud_client::model::Role> for Role {
    fn from(value: golem_cloud_client::model::Role) -> Self {
        match value {
            golem_cloud_client::model::Role::Admin => Role::Admin,
            golem_cloud_client::model::Role::MarketingAdmin => Role::MarketingAdmin,
            golem_cloud_client::model::Role::ViewProject => Role::ViewProject,
            golem_cloud_client::model::Role::DeleteProject => Role::DeleteProject,
            golem_cloud_client::model::Role::CreateProject => Role::CreateProject,
            golem_cloud_client::model::Role::InstanceServer => Role::InstanceServer,
            golem_cloud_client::model::Role::UpdateProject => Role::UpdateProject,
            golem_cloud_client::model::Role::ViewPlugin => Role::ViewPlugin,
            golem_cloud_client::model::Role::CreatePlugin => Role::CreatePlugin,
            golem_cloud_client::model::Role::DeletePlugin => Role::DeletePlugin,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
pub enum ProjectAction {
    ViewComponent,
    CreateComponent,
    UpdateComponent,
    DeleteComponent,
    ViewWorker,
    CreateWorker,
    UpdateWorker,
    DeleteWorker,
    ViewProjectGrants,
    CreateProjectGrants,
    DeleteProjectGrants,
}

impl Display for ProjectAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProjectAction::ViewComponent => "ViewComponent",
            ProjectAction::CreateComponent => "CreateComponent",
            ProjectAction::UpdateComponent => "UpdateComponent",
            ProjectAction::DeleteComponent => "DeleteComponent",
            ProjectAction::ViewWorker => "ViewWorker",
            ProjectAction::CreateWorker => "CreateWorker",
            ProjectAction::UpdateWorker => "UpdateWorker",
            ProjectAction::DeleteWorker => "DeleteWorker",
            ProjectAction::ViewProjectGrants => "ViewProjectGrants",
            ProjectAction::CreateProjectGrants => "CreateProjectGrants",
            ProjectAction::DeleteProjectGrants => "DeleteProjectGrants",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for ProjectAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ViewComponent" => Ok(ProjectAction::ViewComponent),
            "CreateComponent" => Ok(ProjectAction::CreateComponent),
            "UpdateComponent" => Ok(ProjectAction::UpdateComponent),
            "DeleteComponent" => Ok(ProjectAction::DeleteComponent),
            "ViewWorker" => Ok(ProjectAction::ViewWorker),
            "CreateWorker" => Ok(ProjectAction::CreateWorker),
            "UpdateWorker" => Ok(ProjectAction::UpdateWorker),
            "DeleteWorker" => Ok(ProjectAction::DeleteWorker),
            "ViewProjectGrants" => Ok(ProjectAction::ViewProjectGrants),
            "CreateProjectGrants" => Ok(ProjectAction::CreateProjectGrants),
            "DeleteProjectGrants" => Ok(ProjectAction::DeleteProjectGrants),
            _ => {
                let all = ProjectAction::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown action: {s}. Expected one of {all}"))
            }
        }
    }
}

impl From<ProjectAction> for golem_cloud_client::model::ProjectAction {
    fn from(value: ProjectAction) -> Self {
        match value {
            ProjectAction::ViewComponent => golem_cloud_client::model::ProjectAction::ViewComponent,
            ProjectAction::CreateComponent => {
                golem_cloud_client::model::ProjectAction::CreateComponent
            }
            ProjectAction::UpdateComponent => {
                golem_cloud_client::model::ProjectAction::UpdateComponent
            }
            ProjectAction::DeleteComponent => {
                golem_cloud_client::model::ProjectAction::DeleteComponent
            }
            ProjectAction::ViewWorker => golem_cloud_client::model::ProjectAction::ViewWorker,
            ProjectAction::CreateWorker => golem_cloud_client::model::ProjectAction::CreateWorker,
            ProjectAction::UpdateWorker => golem_cloud_client::model::ProjectAction::UpdateWorker,
            ProjectAction::DeleteWorker => golem_cloud_client::model::ProjectAction::DeleteWorker,
            ProjectAction::ViewProjectGrants => {
                golem_cloud_client::model::ProjectAction::ViewProjectGrants
            }
            ProjectAction::CreateProjectGrants => {
                golem_cloud_client::model::ProjectAction::CreateProjectGrants
            }
            ProjectAction::DeleteProjectGrants => {
                golem_cloud_client::model::ProjectAction::DeleteProjectGrants
            }
        }
    }
}
