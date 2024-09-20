pub mod text;

use clap::{ArgMatches, Error, FromArgMatches};
use derive_more::{Display, FromStr, Into};
use golem_cli::command::ComponentRefSplit;
use golem_cli::model::ComponentName;
use golem_cloud_client::model::{Project, ProjectType};
use golem_common::model::ProjectId;
use golem_common::uri::cloud::uri::{ComponentUri, ProjectUri, ToOssUri};
use golem_common::uri::cloud::url::ProjectUrl;
use golem_common::uri::cloud::urn::ProjectUrn;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
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

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize)]
pub enum Role {
    Admin,
    MarketingAdmin,
    ViewProject,
    DeleteProject,
    CreateProject,
    InstanceServer,
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

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ProjectPolicyId(pub Uuid);

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
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

pub mod to_oss {
    use std::collections::HashMap;
    use std::hash::Hash;

    pub trait ToOss<T> {
        fn to_oss(self) -> T;
    }

    impl<A: ToOss<B>, B> ToOss<Box<B>> for Box<A> {
        fn to_oss(self) -> Box<B> {
            Box::new((*self).to_oss())
        }
    }

    impl<A: ToOss<B>, B> ToOss<Option<B>> for Option<A> {
        fn to_oss(self) -> Option<B> {
            self.map(|v| v.to_oss())
        }
    }

    impl<A: ToOss<B>, B> ToOss<Vec<B>> for Vec<A> {
        fn to_oss(self) -> Vec<B> {
            self.into_iter().map(|v| v.to_oss()).collect()
        }
    }

    impl<K: Eq + Hash, A: ToOss<B>, B> ToOss<HashMap<K, B>> for HashMap<K, A> {
        fn to_oss(self) -> HashMap<K, B> {
            self.into_iter().map(|(k, v)| (k, v.to_oss())).collect()
        }
    }

    impl ToOss<golem_client::model::WorkerId> for golem_cloud_client::model::WorkerId {
        fn to_oss(self) -> golem_client::model::WorkerId {
            golem_client::model::WorkerId {
                component_id: self.component_id,
                worker_name: self.worker_name,
            }
        }
    }

    impl ToOss<golem_client::model::ScanCursor> for golem_cloud_client::model::ScanCursor {
        fn to_oss(self) -> golem_client::model::ScanCursor {
            golem_client::model::ScanCursor {
                cursor: self.cursor,
                layer: self.layer,
            }
        }
    }

    impl ToOss<golem_client::model::WorkerStatus> for golem_cloud_client::model::WorkerStatus {
        fn to_oss(self) -> golem_client::model::WorkerStatus {
            match self {
                golem_cloud_client::model::WorkerStatus::Running => {
                    golem_client::model::WorkerStatus::Running
                }
                golem_cloud_client::model::WorkerStatus::Idle => {
                    golem_client::model::WorkerStatus::Idle
                }
                golem_cloud_client::model::WorkerStatus::Suspended => {
                    golem_client::model::WorkerStatus::Suspended
                }
                golem_cloud_client::model::WorkerStatus::Interrupted => {
                    golem_client::model::WorkerStatus::Interrupted
                }
                golem_cloud_client::model::WorkerStatus::Retrying => {
                    golem_client::model::WorkerStatus::Retrying
                }
                golem_cloud_client::model::WorkerStatus::Failed => {
                    golem_client::model::WorkerStatus::Failed
                }
                golem_cloud_client::model::WorkerStatus::Exited => {
                    golem_client::model::WorkerStatus::Exited
                }
            }
        }
    }

    impl ToOss<golem_client::model::PendingUpdate> for golem_cloud_client::model::PendingUpdate {
        fn to_oss(self) -> golem_client::model::PendingUpdate {
            golem_client::model::PendingUpdate {
                timestamp: self.timestamp,
                target_version: self.target_version,
            }
        }
    }

    impl ToOss<golem_client::model::SuccessfulUpdate> for golem_cloud_client::model::SuccessfulUpdate {
        fn to_oss(self) -> golem_client::model::SuccessfulUpdate {
            golem_client::model::SuccessfulUpdate {
                timestamp: self.timestamp,
                target_version: self.target_version,
            }
        }
    }

    impl ToOss<golem_client::model::FailedUpdate> for golem_cloud_client::model::FailedUpdate {
        fn to_oss(self) -> golem_client::model::FailedUpdate {
            golem_client::model::FailedUpdate {
                timestamp: self.timestamp,
                target_version: self.target_version,
                details: self.details,
            }
        }
    }

    impl ToOss<golem_client::model::UpdateRecord> for golem_cloud_client::model::UpdateRecord {
        fn to_oss(self) -> golem_client::model::UpdateRecord {
            match self {
                golem_cloud_client::model::UpdateRecord::PendingUpdate(u) => {
                    golem_client::model::UpdateRecord::PendingUpdate(u.to_oss())
                }
                golem_cloud_client::model::UpdateRecord::SuccessfulUpdate(u) => {
                    golem_client::model::UpdateRecord::SuccessfulUpdate(u.to_oss())
                }
                golem_cloud_client::model::UpdateRecord::FailedUpdate(u) => {
                    golem_client::model::UpdateRecord::FailedUpdate(u.to_oss())
                }
            }
        }
    }

    impl ToOss<golem_client::model::IndexedWorkerMetadata>
        for golem_cloud_client::model::IndexedWorkerMetadata
    {
        fn to_oss(self) -> golem_client::model::IndexedWorkerMetadata {
            golem_client::model::IndexedWorkerMetadata {
                resource_name: self.resource_name,
                resource_params: self.resource_params,
            }
        }
    }

    impl ToOss<golem_client::model::ResourceMetadata> for golem_cloud_client::model::ResourceMetadata {
        fn to_oss(self) -> golem_client::model::ResourceMetadata {
            golem_client::model::ResourceMetadata {
                created_at: self.created_at,
                indexed: self.indexed.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::ApiDefinitionInfo>
        for golem_cloud_client::model::ApiDefinitionInfo
    {
        fn to_oss(self) -> golem_client::model::ApiDefinitionInfo {
            let golem_cloud_client::model::ApiDefinitionInfo { id, version } = self;

            golem_client::model::ApiDefinitionInfo { id, version }
        }
    }

    impl ToOss<golem_client::model::ApiSite> for golem_cloud_client::model::ApiSite {
        fn to_oss(self) -> golem_client::model::ApiSite {
            let golem_cloud_client::model::ApiSite { host, subdomain } = self;
            golem_client::model::ApiSite { host, subdomain }
        }
    }

    impl ToOss<golem_client::model::VersionedComponentId>
        for golem_cloud_client::model::VersionedComponentId
    {
        fn to_oss(self) -> golem_client::model::VersionedComponentId {
            golem_client::model::VersionedComponentId {
                component_id: self.component_id,
                version: self.version,
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedType> for golem_cloud_client::model::AnalysedType {
        fn to_oss(self) -> golem_client::model::AnalysedType {
            match self {
                golem_cloud_client::model::AnalysedType::Variant(x) => {
                    golem_client::model::AnalysedType::Variant(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Result(x) => {
                    golem_client::model::AnalysedType::Result(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Option(x) => {
                    golem_client::model::AnalysedType::Option(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Enum(x) => {
                    golem_client::model::AnalysedType::Enum(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Flags(x) => {
                    golem_client::model::AnalysedType::Flags(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Record(x) => {
                    golem_client::model::AnalysedType::Record(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Tuple(x) => {
                    golem_client::model::AnalysedType::Tuple(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::List(x) => {
                    golem_client::model::AnalysedType::List(x.to_oss())
                }
                golem_cloud_client::model::AnalysedType::Str(_) => {
                    golem_client::model::AnalysedType::Str(golem_client::model::TypeStr {})
                }
                golem_cloud_client::model::AnalysedType::Chr(_) => {
                    golem_client::model::AnalysedType::Chr(golem_client::model::TypeChr {})
                }
                golem_cloud_client::model::AnalysedType::F64(_) => {
                    golem_client::model::AnalysedType::F64(golem_client::model::TypeF64 {})
                }
                golem_cloud_client::model::AnalysedType::F32(_) => {
                    golem_client::model::AnalysedType::F32(golem_client::model::TypeF32 {})
                }
                golem_cloud_client::model::AnalysedType::U64(_) => {
                    golem_client::model::AnalysedType::U64(golem_client::model::TypeU64 {})
                }
                golem_cloud_client::model::AnalysedType::S64(_) => {
                    golem_client::model::AnalysedType::S64(golem_client::model::TypeS64 {})
                }
                golem_cloud_client::model::AnalysedType::U32(_) => {
                    golem_client::model::AnalysedType::U32(golem_client::model::TypeU32 {})
                }
                golem_cloud_client::model::AnalysedType::S32(_) => {
                    golem_client::model::AnalysedType::S32(golem_client::model::TypeS32 {})
                }
                golem_cloud_client::model::AnalysedType::U16(_) => {
                    golem_client::model::AnalysedType::U16(golem_client::model::TypeU16 {})
                }
                golem_cloud_client::model::AnalysedType::S16(_) => {
                    golem_client::model::AnalysedType::S16(golem_client::model::TypeS16 {})
                }
                golem_cloud_client::model::AnalysedType::U8(_) => {
                    golem_client::model::AnalysedType::U8(golem_client::model::TypeU8 {})
                }
                golem_cloud_client::model::AnalysedType::S8(_) => {
                    golem_client::model::AnalysedType::S8(golem_client::model::TypeS8 {})
                }
                golem_cloud_client::model::AnalysedType::Bool(_) => {
                    golem_client::model::AnalysedType::Bool(golem_client::model::TypeBool {})
                }
                golem_cloud_client::model::AnalysedType::Handle(x) => {
                    golem_client::model::AnalysedType::Handle(x.to_oss())
                }
            }
        }
    }

    impl ToOss<golem_client::model::NameOptionTypePair>
        for golem_cloud_client::model::NameOptionTypePair
    {
        fn to_oss(self) -> golem_client::model::NameOptionTypePair {
            golem_client::model::NameOptionTypePair {
                name: self.name,
                typ: self.typ.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeVariant> for golem_cloud_client::model::TypeVariant {
        fn to_oss(self) -> golem_client::model::TypeVariant {
            golem_client::model::TypeVariant {
                cases: self.cases.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeResult> for golem_cloud_client::model::TypeResult {
        fn to_oss(self) -> golem_client::model::TypeResult {
            golem_client::model::TypeResult {
                ok: self.ok.to_oss(),
                err: self.err.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeOption> for golem_cloud_client::model::TypeOption {
        fn to_oss(self) -> golem_client::model::TypeOption {
            golem_client::model::TypeOption {
                inner: self.inner.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeEnum> for golem_cloud_client::model::TypeEnum {
        fn to_oss(self) -> golem_client::model::TypeEnum {
            golem_client::model::TypeEnum { cases: self.cases }
        }
    }

    impl ToOss<golem_client::model::TypeFlags> for golem_cloud_client::model::TypeFlags {
        fn to_oss(self) -> golem_client::model::TypeFlags {
            golem_client::model::TypeFlags { names: self.names }
        }
    }

    impl ToOss<golem_client::model::NameTypePair> for golem_cloud_client::model::NameTypePair {
        fn to_oss(self) -> golem_client::model::NameTypePair {
            golem_client::model::NameTypePair {
                name: self.name,
                typ: self.typ.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeRecord> for golem_cloud_client::model::TypeRecord {
        fn to_oss(self) -> golem_client::model::TypeRecord {
            golem_client::model::TypeRecord {
                fields: self.fields.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeTuple> for golem_cloud_client::model::TypeTuple {
        fn to_oss(self) -> golem_client::model::TypeTuple {
            golem_client::model::TypeTuple {
                items: self.items.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeList> for golem_cloud_client::model::TypeList {
        fn to_oss(self) -> golem_client::model::TypeList {
            golem_client::model::TypeList {
                inner: self.inner.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::TypeAnnotatedValue>
        for golem_cloud_client::model::TypeAnnotatedValue
    {
        fn to_oss(self) -> golem_client::model::TypeAnnotatedValue {
            golem_client::model::TypeAnnotatedValue {
                typ: self.typ.to_oss(),
                value: self.value,
            }
        }
    }

    impl ToOss<golem_client::model::InvokeParameters> for golem_cloud_client::model::InvokeParameters {
        fn to_oss(self) -> golem_client::model::InvokeParameters {
            golem_client::model::InvokeParameters {
                params: self.params.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::InvokeResult> for golem_cloud_client::model::InvokeResult {
        fn to_oss(self) -> golem_client::model::InvokeResult {
            golem_client::model::InvokeResult {
                result: self.result.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedResourceMode>
        for golem_cloud_client::model::AnalysedResourceMode
    {
        fn to_oss(self) -> golem_client::model::AnalysedResourceMode {
            match self {
                golem_cloud_client::model::AnalysedResourceMode::Borrowed => {
                    golem_client::model::AnalysedResourceMode::Borrowed
                }
                golem_cloud_client::model::AnalysedResourceMode::Owned => {
                    golem_client::model::AnalysedResourceMode::Owned
                }
            }
        }
    }

    impl ToOss<golem_client::model::TypeHandle> for golem_cloud_client::model::TypeHandle {
        fn to_oss(self) -> golem_client::model::TypeHandle {
            golem_client::model::TypeHandle {
                resource_id: self.resource_id,
                mode: self.mode.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedFunctionParameter>
        for golem_cloud_client::model::AnalysedFunctionParameter
    {
        fn to_oss(self) -> golem_client::model::AnalysedFunctionParameter {
            golem_client::model::AnalysedFunctionParameter {
                name: self.name,
                typ: self.typ.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedFunctionResult>
        for golem_cloud_client::model::AnalysedFunctionResult
    {
        fn to_oss(self) -> golem_client::model::AnalysedFunctionResult {
            golem_client::model::AnalysedFunctionResult {
                name: self.name,
                typ: self.typ.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedFunction> for golem_cloud_client::model::AnalysedFunction {
        fn to_oss(self) -> golem_client::model::AnalysedFunction {
            golem_client::model::AnalysedFunction {
                name: self.name,
                parameters: self.parameters.to_oss(),
                results: self.results.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedInstance> for golem_cloud_client::model::AnalysedInstance {
        fn to_oss(self) -> golem_client::model::AnalysedInstance {
            golem_client::model::AnalysedInstance {
                name: self.name,
                functions: self.functions.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::AnalysedExport> for golem_cloud_client::model::AnalysedExport {
        fn to_oss(self) -> golem_client::model::AnalysedExport {
            match self {
                golem_cloud_client::model::AnalysedExport::Instance(i) => {
                    golem_client::model::AnalysedExport::Instance(i.to_oss())
                }
                golem_cloud_client::model::AnalysedExport::Function(f) => {
                    golem_client::model::AnalysedExport::Function(f.to_oss())
                }
            }
        }
    }

    impl ToOss<golem_client::model::VersionedName> for golem_cloud_client::model::VersionedName {
        fn to_oss(self) -> golem_client::model::VersionedName {
            golem_client::model::VersionedName {
                name: self.name,
                version: self.version,
            }
        }
    }

    impl ToOss<golem_client::model::ProducerField> for golem_cloud_client::model::ProducerField {
        fn to_oss(self) -> golem_client::model::ProducerField {
            golem_client::model::ProducerField {
                name: self.name,
                values: self.values.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::Producers> for golem_cloud_client::model::Producers {
        fn to_oss(self) -> golem_client::model::Producers {
            golem_client::model::Producers {
                fields: self.fields.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::LinearMemory> for golem_cloud_client::model::LinearMemory {
        fn to_oss(self) -> golem_client::model::LinearMemory {
            golem_client::model::LinearMemory {
                initial: self.initial,
                maximum: self.maximum,
            }
        }
    }

    impl ToOss<golem_client::model::ComponentMetadata>
        for golem_cloud_client::model::ComponentMetadata
    {
        fn to_oss(self) -> golem_client::model::ComponentMetadata {
            golem_client::model::ComponentMetadata {
                exports: self.exports.to_oss(),
                producers: self.producers.to_oss(),
                memories: self.memories.to_oss(),
            }
        }
    }

    impl ToOss<golem_client::model::ComponentType> for golem_cloud_client::model::ComponentType {
        fn to_oss(self) -> golem_client::model::ComponentType {
            match self {
                golem_cloud_client::model::ComponentType::Durable => {
                    golem_client::model::ComponentType::Durable
                }
                golem_cloud_client::model::ComponentType::Ephemeral => {
                    golem_client::model::ComponentType::Ephemeral
                }
            }
        }
    }
}

pub mod to_cloud {
    pub trait ToCloud<T> {
        fn to_cloud(self) -> T;
    }

    impl<A: ToCloud<B>, B> ToCloud<Box<B>> for Box<A> {
        fn to_cloud(self) -> Box<B> {
            Box::new((*self).to_cloud())
        }
    }

    impl<A: ToCloud<B>, B> ToCloud<Option<B>> for Option<A> {
        fn to_cloud(self) -> Option<B> {
            self.map(|v| v.to_cloud())
        }
    }

    impl<A: ToCloud<B>, B> ToCloud<Vec<B>> for Vec<A> {
        fn to_cloud(self) -> Vec<B> {
            self.into_iter().map(|v| v.to_cloud()).collect()
        }
    }

    impl ToCloud<golem_cloud_client::model::ComponentType> for golem_client::model::ComponentType {
        fn to_cloud(self) -> golem_cloud_client::model::ComponentType {
            match self {
                golem_client::model::ComponentType::Durable => {
                    golem_cloud_client::model::ComponentType::Durable
                }
                golem_client::model::ComponentType::Ephemeral => {
                    golem_cloud_client::model::ComponentType::Ephemeral
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::ScanCursor> for golem_client::model::ScanCursor {
        fn to_cloud(self) -> golem_cloud_client::model::ScanCursor {
            golem_cloud_client::model::ScanCursor {
                cursor: self.cursor,
                layer: self.layer,
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::AnalysedType> for golem_client::model::AnalysedType {
        fn to_cloud(self) -> golem_cloud_client::model::AnalysedType {
            match self {
                golem_client::model::AnalysedType::Variant(x) => {
                    golem_cloud_client::model::AnalysedType::Variant(x.to_cloud())
                }
                golem_client::model::AnalysedType::Result(x) => {
                    golem_cloud_client::model::AnalysedType::Result(x.to_cloud())
                }
                golem_client::model::AnalysedType::Option(x) => {
                    golem_cloud_client::model::AnalysedType::Option(x.to_cloud())
                }
                golem_client::model::AnalysedType::Enum(x) => {
                    golem_cloud_client::model::AnalysedType::Enum(x.to_cloud())
                }
                golem_client::model::AnalysedType::Flags(x) => {
                    golem_cloud_client::model::AnalysedType::Flags(x.to_cloud())
                }
                golem_client::model::AnalysedType::Record(x) => {
                    golem_cloud_client::model::AnalysedType::Record(x.to_cloud())
                }
                golem_client::model::AnalysedType::Tuple(x) => {
                    golem_cloud_client::model::AnalysedType::Tuple(x.to_cloud())
                }
                golem_client::model::AnalysedType::List(x) => {
                    golem_cloud_client::model::AnalysedType::List(x.to_cloud())
                }
                golem_client::model::AnalysedType::Str(_) => {
                    golem_cloud_client::model::AnalysedType::Str(
                        golem_cloud_client::model::TypeStr {},
                    )
                }
                golem_client::model::AnalysedType::Chr(_) => {
                    golem_cloud_client::model::AnalysedType::Chr(
                        golem_cloud_client::model::TypeChr {},
                    )
                }
                golem_client::model::AnalysedType::F64(_) => {
                    golem_cloud_client::model::AnalysedType::F64(
                        golem_cloud_client::model::TypeF64 {},
                    )
                }
                golem_client::model::AnalysedType::F32(_) => {
                    golem_cloud_client::model::AnalysedType::F32(
                        golem_cloud_client::model::TypeF32 {},
                    )
                }
                golem_client::model::AnalysedType::U64(_) => {
                    golem_cloud_client::model::AnalysedType::U64(
                        golem_cloud_client::model::TypeU64 {},
                    )
                }
                golem_client::model::AnalysedType::S64(_) => {
                    golem_cloud_client::model::AnalysedType::S64(
                        golem_cloud_client::model::TypeS64 {},
                    )
                }
                golem_client::model::AnalysedType::U32(_) => {
                    golem_cloud_client::model::AnalysedType::U32(
                        golem_cloud_client::model::TypeU32 {},
                    )
                }
                golem_client::model::AnalysedType::S32(_) => {
                    golem_cloud_client::model::AnalysedType::S32(
                        golem_cloud_client::model::TypeS32 {},
                    )
                }
                golem_client::model::AnalysedType::U16(_) => {
                    golem_cloud_client::model::AnalysedType::U16(
                        golem_cloud_client::model::TypeU16 {},
                    )
                }
                golem_client::model::AnalysedType::S16(_) => {
                    golem_cloud_client::model::AnalysedType::S16(
                        golem_cloud_client::model::TypeS16 {},
                    )
                }
                golem_client::model::AnalysedType::U8(_) => {
                    golem_cloud_client::model::AnalysedType::U8(
                        golem_cloud_client::model::TypeU8 {},
                    )
                }
                golem_client::model::AnalysedType::S8(_) => {
                    golem_cloud_client::model::AnalysedType::S8(
                        golem_cloud_client::model::TypeS8 {},
                    )
                }
                golem_client::model::AnalysedType::Bool(_) => {
                    golem_cloud_client::model::AnalysedType::Bool(
                        golem_cloud_client::model::TypeBool {},
                    )
                }
                golem_client::model::AnalysedType::Handle(x) => {
                    golem_cloud_client::model::AnalysedType::Handle(x.to_cloud())
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::NameOptionTypePair>
        for golem_client::model::NameOptionTypePair
    {
        fn to_cloud(self) -> golem_cloud_client::model::NameOptionTypePair {
            golem_cloud_client::model::NameOptionTypePair {
                name: self.name,
                typ: self.typ.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeVariant> for golem_client::model::TypeVariant {
        fn to_cloud(self) -> golem_cloud_client::model::TypeVariant {
            golem_cloud_client::model::TypeVariant {
                cases: self.cases.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeResult> for golem_client::model::TypeResult {
        fn to_cloud(self) -> golem_cloud_client::model::TypeResult {
            golem_cloud_client::model::TypeResult {
                ok: self.ok.to_cloud(),
                err: self.err.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeOption> for golem_client::model::TypeOption {
        fn to_cloud(self) -> golem_cloud_client::model::TypeOption {
            golem_cloud_client::model::TypeOption {
                inner: self.inner.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeEnum> for golem_client::model::TypeEnum {
        fn to_cloud(self) -> golem_cloud_client::model::TypeEnum {
            golem_cloud_client::model::TypeEnum { cases: self.cases }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeFlags> for golem_client::model::TypeFlags {
        fn to_cloud(self) -> golem_cloud_client::model::TypeFlags {
            golem_cloud_client::model::TypeFlags { names: self.names }
        }
    }

    impl ToCloud<golem_cloud_client::model::NameTypePair> for golem_client::model::NameTypePair {
        fn to_cloud(self) -> golem_cloud_client::model::NameTypePair {
            golem_cloud_client::model::NameTypePair {
                name: self.name,
                typ: self.typ.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeRecord> for golem_client::model::TypeRecord {
        fn to_cloud(self) -> golem_cloud_client::model::TypeRecord {
            golem_cloud_client::model::TypeRecord {
                fields: self.fields.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeTuple> for golem_client::model::TypeTuple {
        fn to_cloud(self) -> golem_cloud_client::model::TypeTuple {
            golem_cloud_client::model::TypeTuple {
                items: self.items.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeList> for golem_client::model::TypeList {
        fn to_cloud(self) -> golem_cloud_client::model::TypeList {
            golem_cloud_client::model::TypeList {
                inner: self.inner.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeAnnotatedValue>
        for golem_client::model::TypeAnnotatedValue
    {
        fn to_cloud(self) -> golem_cloud_client::model::TypeAnnotatedValue {
            golem_cloud_client::model::TypeAnnotatedValue {
                typ: self.typ.to_cloud(),
                value: self.value,
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::InvokeParameters>
        for golem_client::model::InvokeParameters
    {
        fn to_cloud(self) -> golem_cloud_client::model::InvokeParameters {
            golem_cloud_client::model::InvokeParameters {
                params: self.params.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::AnalysedResourceMode>
        for golem_client::model::AnalysedResourceMode
    {
        fn to_cloud(self) -> golem_cloud_client::model::AnalysedResourceMode {
            match self {
                golem_client::model::AnalysedResourceMode::Borrowed => {
                    golem_cloud_client::model::AnalysedResourceMode::Borrowed
                }
                golem_client::model::AnalysedResourceMode::Owned => {
                    golem_cloud_client::model::AnalysedResourceMode::Owned
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::TypeHandle> for golem_client::model::TypeHandle {
        fn to_cloud(self) -> golem_cloud_client::model::TypeHandle {
            golem_cloud_client::model::TypeHandle {
                resource_id: self.resource_id,
                mode: self.mode.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerFilter> for golem_client::model::WorkerFilter {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerFilter {
            match self {
                golem_client::model::WorkerFilter::Name(v) => {
                    golem_cloud_client::model::WorkerFilter::Name(v.to_cloud())
                }
                golem_client::model::WorkerFilter::Status(v) => {
                    golem_cloud_client::model::WorkerFilter::Status(v.to_cloud())
                }
                golem_client::model::WorkerFilter::Version(v) => {
                    golem_cloud_client::model::WorkerFilter::Version(v.to_cloud())
                }
                golem_client::model::WorkerFilter::CreatedAt(v) => {
                    golem_cloud_client::model::WorkerFilter::CreatedAt(v.to_cloud())
                }
                golem_client::model::WorkerFilter::Env(v) => {
                    golem_cloud_client::model::WorkerFilter::Env(v.to_cloud())
                }
                golem_client::model::WorkerFilter::And(v) => {
                    golem_cloud_client::model::WorkerFilter::And(v.to_cloud())
                }
                golem_client::model::WorkerFilter::Or(v) => {
                    golem_cloud_client::model::WorkerFilter::Or(v.to_cloud())
                }
                golem_client::model::WorkerFilter::Not(v) => {
                    golem_cloud_client::model::WorkerFilter::Not(v.to_cloud())
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::StringFilterComparator>
        for golem_client::model::StringFilterComparator
    {
        fn to_cloud(self) -> golem_cloud_client::model::StringFilterComparator {
            match self {
                golem_client::model::StringFilterComparator::Equal => {
                    golem_cloud_client::model::StringFilterComparator::Equal
                }
                golem_client::model::StringFilterComparator::NotEqual => {
                    golem_cloud_client::model::StringFilterComparator::NotEqual
                }
                golem_client::model::StringFilterComparator::Like => {
                    golem_cloud_client::model::StringFilterComparator::Like
                }
                golem_client::model::StringFilterComparator::NotLike => {
                    golem_cloud_client::model::StringFilterComparator::NotLike
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerNameFilter>
        for golem_client::model::WorkerNameFilter
    {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerNameFilter {
            golem_cloud_client::model::WorkerNameFilter {
                comparator: self.comparator.to_cloud(),
                value: self.value,
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::FilterComparator>
        for golem_client::model::FilterComparator
    {
        fn to_cloud(self) -> golem_cloud_client::model::FilterComparator {
            match self {
                golem_client::model::FilterComparator::Equal => {
                    golem_cloud_client::model::FilterComparator::Equal
                }
                golem_client::model::FilterComparator::NotEqual => {
                    golem_cloud_client::model::FilterComparator::NotEqual
                }
                golem_client::model::FilterComparator::GreaterEqual => {
                    golem_cloud_client::model::FilterComparator::GreaterEqual
                }
                golem_client::model::FilterComparator::Greater => {
                    golem_cloud_client::model::FilterComparator::Greater
                }
                golem_client::model::FilterComparator::LessEqual => {
                    golem_cloud_client::model::FilterComparator::LessEqual
                }
                golem_client::model::FilterComparator::Less => {
                    golem_cloud_client::model::FilterComparator::Less
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerStatus> for golem_client::model::WorkerStatus {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerStatus {
            match self {
                golem_client::model::WorkerStatus::Running => {
                    golem_cloud_client::model::WorkerStatus::Running
                }
                golem_client::model::WorkerStatus::Idle => {
                    golem_cloud_client::model::WorkerStatus::Idle
                }
                golem_client::model::WorkerStatus::Suspended => {
                    golem_cloud_client::model::WorkerStatus::Suspended
                }
                golem_client::model::WorkerStatus::Interrupted => {
                    golem_cloud_client::model::WorkerStatus::Interrupted
                }
                golem_client::model::WorkerStatus::Retrying => {
                    golem_cloud_client::model::WorkerStatus::Retrying
                }
                golem_client::model::WorkerStatus::Failed => {
                    golem_cloud_client::model::WorkerStatus::Failed
                }
                golem_client::model::WorkerStatus::Exited => {
                    golem_cloud_client::model::WorkerStatus::Exited
                }
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerStatusFilter>
        for golem_client::model::WorkerStatusFilter
    {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerStatusFilter {
            golem_cloud_client::model::WorkerStatusFilter {
                comparator: self.comparator.to_cloud(),
                value: self.value.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerVersionFilter>
        for golem_client::model::WorkerVersionFilter
    {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerVersionFilter {
            golem_cloud_client::model::WorkerVersionFilter {
                comparator: self.comparator.to_cloud(),
                value: self.value,
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerCreatedAtFilter>
        for golem_client::model::WorkerCreatedAtFilter
    {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerCreatedAtFilter {
            golem_cloud_client::model::WorkerCreatedAtFilter {
                comparator: self.comparator.to_cloud(),
                value: self.value,
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerEnvFilter> for golem_client::model::WorkerEnvFilter {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerEnvFilter {
            golem_cloud_client::model::WorkerEnvFilter {
                name: self.name,
                comparator: self.comparator.to_cloud(),
                value: self.value,
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerAndFilter> for golem_client::model::WorkerAndFilter {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerAndFilter {
            golem_cloud_client::model::WorkerAndFilter {
                filters: self.filters.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerOrFilter> for golem_client::model::WorkerOrFilter {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerOrFilter {
            golem_cloud_client::model::WorkerOrFilter {
                filters: self.filters.to_cloud(),
            }
        }
    }

    impl ToCloud<golem_cloud_client::model::WorkerNotFilter> for golem_client::model::WorkerNotFilter {
        fn to_cloud(self) -> golem_cloud_client::model::WorkerNotFilter {
            golem_cloud_client::model::WorkerNotFilter {
                filter: self.filter.to_cloud(),
            }
        }
    }
}

pub mod to_cli {
    use crate::cloud::model::to_oss::ToOss;
    use golem_cli::model::component::Component;

    pub trait ToCli<T> {
        fn to_cli(self) -> T;
    }

    impl<A: ToCli<B>, B> ToCli<Option<B>> for Option<A> {
        fn to_cli(self) -> Option<B> {
            self.map(|v| v.to_cli())
        }
    }

    impl<A: ToCli<B>, B> ToCli<Vec<B>> for Vec<A> {
        fn to_cli(self) -> Vec<B> {
            self.into_iter().map(|v| v.to_cli()).collect()
        }
    }

    impl ToCli<golem_cli::model::WorkerMetadata> for golem_cloud_client::model::WorkerMetadata {
        fn to_cli(self) -> golem_cli::model::WorkerMetadata {
            golem_cli::model::WorkerMetadata {
                worker_id: self.worker_id.to_oss(),
                account_id: Some(golem_cli::cloud::AccountId {
                    id: self.account_id,
                }),
                args: self.args,
                env: self.env,
                status: self.status.to_oss(),
                component_version: self.component_version,
                retry_count: self.retry_count,
                pending_invocation_count: self.pending_invocation_count,
                updates: self.updates.to_oss(),
                created_at: self.created_at,
                last_error: self.last_error,
                component_size: self.component_size,
                total_linear_memory_size: self.total_linear_memory_size,
                owned_resources: self.owned_resources.to_oss(),
            }
        }
    }

    impl ToCli<golem_cli::model::WorkersMetadataResponse>
        for golem_cloud_client::model::WorkersMetadataResponse
    {
        fn to_cli(self) -> golem_cli::model::WorkersMetadataResponse {
            golem_cli::model::WorkersMetadataResponse {
                cursor: self.cursor.to_oss(),
                workers: self.workers.to_cli(),
            }
        }
    }

    impl ToCli<golem_cli::model::ApiDeployment> for golem_cloud_client::model::ApiDeployment {
        fn to_cli(self) -> golem_cli::model::ApiDeployment {
            golem_cli::model::ApiDeployment {
                api_definitions: self.api_definitions.to_oss(),
                project_id: Some(self.project_id),
                site: self.site.to_oss(),
                created_at: self.created_at,
            }
        }
    }

    impl ToCli<Component> for golem_cloud_client::model::Component {
        fn to_cli(self) -> Component {
            Component {
                versioned_component_id: self.versioned_component_id.to_oss(),
                component_name: self.component_name,
                component_size: self.component_size,
                metadata: self.metadata.to_oss(),
                project_id: Some(golem_cli::cloud::ProjectId(self.project_id)),
                created_at: self.created_at,
                component_type: self
                    .component_type
                    .to_oss()
                    .unwrap_or(golem_client::model::ComponentType::Durable),
            }
        }
    }
}
