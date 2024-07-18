pub mod text;

use clap::{ArgMatches, Error, FromArgMatches};
use derive_more::{Display, FromStr, Into};
use golem_cli::cloud::{AccountId, ProjectId};
use golem_cli::command::ComponentRefSplit;
use golem_cli::model::component::Component;
use golem_cli::model::{
    ApiDeployment, ComponentId, ComponentIdOrName, ComponentName, WorkerMetadata,
    WorkersMetadataResponse,
};
use golem_client::model::{IndexedWorkerMetadata, ResourceMetadata};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Into)]
pub struct TokenId(pub Uuid);

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProjectRef {
    Id(ProjectId),
    Name(String),
    Default,
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
struct ProjectRefArgs {
    #[arg(short = 'P', long, conflicts_with = "project_name")]
    project_id: Option<Uuid>,

    #[arg(short = 'p', long, conflicts_with = "project_id")]
    project_name: Option<String>,
}

impl From<&ProjectRefArgs> for ProjectRef {
    fn from(value: &ProjectRefArgs) -> ProjectRef {
        if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        }
    }
}

impl From<&ProjectRef> for ProjectRefArgs {
    fn from(value: &ProjectRef) -> Self {
        match value {
            ProjectRef::Id(ProjectId(id)) => ProjectRefArgs {
                project_id: Some(*id),
                project_name: None,
            },
            ProjectRef::Name(name) => ProjectRefArgs {
                project_id: None,
                project_name: Some(name.clone()),
            },
            ProjectRef::Default => ProjectRefArgs {
                project_id: None,
                project_name: None,
            },
        }
    }
}

impl FromArgMatches for CloudComponentIdOrName {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        CloudComponentIdOrNameArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: CloudComponentIdOrNameArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = CloudComponentIdOrNameArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for CloudComponentIdOrName {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        CloudComponentIdOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        CloudComponentIdOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct CloudComponentIdOrNameArgs {
    #[arg(short = 'C', long, conflicts_with = "component_name", required = true)]
    component_id: Option<Uuid>,

    #[arg(short = 'c', long, conflicts_with = "component_id", required = true)]
    component_name: Option<String>,

    #[arg(
        short = 'P',
        long,
        conflicts_with = "project_name",
        conflicts_with = "component_id"
    )]
    project_id: Option<Uuid>,

    #[arg(
        short = 'p',
        long,
        conflicts_with = "project_id",
        conflicts_with = "component_id"
    )]
    project_name: Option<String>,
}

impl From<&CloudComponentIdOrNameArgs> for CloudComponentIdOrName {
    fn from(value: &CloudComponentIdOrNameArgs) -> CloudComponentIdOrName {
        let pr = if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        };

        if let Some(id) = value.component_id {
            CloudComponentIdOrName::Id(ComponentId(id))
        } else {
            CloudComponentIdOrName::Name(
                ComponentName(value.component_name.as_ref().unwrap().to_string()),
                pr,
            )
        }
    }
}

impl From<&CloudComponentIdOrName> for CloudComponentIdOrNameArgs {
    fn from(value: &CloudComponentIdOrName) -> CloudComponentIdOrNameArgs {
        match value {
            CloudComponentIdOrName::Id(ComponentId(id)) => CloudComponentIdOrNameArgs {
                component_id: Some(*id),
                component_name: None,
                project_id: None,
                project_name: None,
            },
            CloudComponentIdOrName::Name(ComponentName(name), pr) => {
                let (project_id, project_name) = match pr {
                    ProjectRef::Id(ProjectId(id)) => (Some(*id), None),
                    ProjectRef::Name(name) => (None, Some(name.to_string())),
                    ProjectRef::Default => (None, None),
                };

                CloudComponentIdOrNameArgs {
                    component_id: None,
                    component_name: Some(name.clone()),
                    project_id,
                    project_name,
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CloudComponentIdOrName {
    Id(ComponentId),
    Name(ComponentName, ProjectRef),
}

impl ComponentRefSplit<ProjectRef> for CloudComponentIdOrName {
    fn split(self) -> (ComponentIdOrName, Option<ProjectRef>) {
        match self {
            CloudComponentIdOrName::Id(id) => (ComponentIdOrName::Id(id), None),
            CloudComponentIdOrName::Name(name, p) => (ComponentIdOrName::Name(name), Some(p)),
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

pub trait ToOss<T> {
    fn to_oss(self) -> T;
}

pub trait ToCloud<T> {
    fn to_cloud(self) -> T;
}

pub trait ToCli<T> {
    fn to_cli(self) -> T;
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

impl ToCloud<golem_cloud_client::model::ScanCursor> for golem_client::model::ScanCursor {
    fn to_cloud(self) -> golem_cloud_client::model::ScanCursor {
        golem_cloud_client::model::ScanCursor {
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

impl ToCli<WorkerMetadata> for golem_cloud_client::model::WorkerMetadata {
    fn to_cli(self) -> WorkerMetadata {
        fn to_oss_indexed_resource(
            m: golem_cloud_client::model::IndexedWorkerMetadata,
        ) -> IndexedWorkerMetadata {
            IndexedWorkerMetadata {
                resource_name: m.resource_name,
                resource_params: m.resource_params,
            }
        }

        fn to_oss_resource(m: golem_cloud_client::model::ResourceMetadata) -> ResourceMetadata {
            ResourceMetadata {
                created_at: m.created_at,
                indexed: m.indexed.map(to_oss_indexed_resource),
            }
        }

        let golem_cloud_client::model::WorkerMetadata {
            worker_id,
            account_id,
            args,
            env,
            status,
            component_version,
            retry_count,
            pending_invocation_count,
            updates,
            created_at,
            last_error,
            component_size,
            total_linear_memory_size,
            owned_resources,
        } = self;

        WorkerMetadata {
            worker_id: worker_id.to_oss(),
            account_id: Some(AccountId { id: account_id }),
            args,
            env,
            status: status.to_oss(),
            component_version,
            retry_count,
            pending_invocation_count,
            updates: updates.into_iter().map(|u| u.to_oss()).collect(),
            created_at,
            last_error,
            component_size,
            total_linear_memory_size,
            owned_resources: owned_resources
                .into_iter()
                .map(|(k, v)| (k, to_oss_resource(v)))
                .collect(),
        }
    }
}

impl ToCli<WorkersMetadataResponse> for golem_cloud_client::model::WorkersMetadataResponse {
    fn to_cli(self) -> WorkersMetadataResponse {
        let golem_cloud_client::model::WorkersMetadataResponse { workers, cursor } = self;

        WorkersMetadataResponse {
            cursor: cursor.map(|c| c.to_oss()),
            workers: workers.into_iter().map(|w| w.to_cli()).collect(),
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

impl ToCli<ApiDeployment> for golem_cloud_client::model::ApiDeployment {
    fn to_cli(self) -> ApiDeployment {
        let golem_cloud_client::model::ApiDeployment {
            api_definitions,
            project_id,
            site,
        } = self;

        ApiDeployment {
            api_definitions: api_definitions.into_iter().map(|d| d.to_oss()).collect(),
            project_id: Some(project_id),
            site: site.to_oss(),
        }
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

impl ToOss<golem_client::model::UserComponentId> for golem_cloud_client::model::UserComponentId {
    fn to_oss(self) -> golem_client::model::UserComponentId {
        golem_client::model::UserComponentId {
            versioned_component_id: self.versioned_component_id.to_oss(),
        }
    }
}

impl ToOss<golem_client::model::ProtectedComponentId>
    for golem_cloud_client::model::ProtectedComponentId
{
    fn to_oss(self) -> golem_client::model::ProtectedComponentId {
        golem_client::model::ProtectedComponentId {
            versioned_component_id: self.versioned_component_id.to_oss(),
        }
    }
}

impl ToOss<golem_client::model::ComponentMetadata>
    for golem_cloud_client::model::ComponentMetadata
{
    fn to_oss(self) -> golem_client::model::ComponentMetadata {
        let golem_cloud_client::model::ComponentMetadata {
            exports,
            producers,
            memories,
        } = self;

        fn to_oss_notp(
            p: golem_cloud_client::model::NameOptionTypePair,
        ) -> golem_client::model::NameOptionTypePair {
            let golem_cloud_client::model::NameOptionTypePair { name, typ } = p;

            golem_client::model::NameOptionTypePair {
                name,
                typ: typ.map(to_oss_type),
            }
        }

        fn to_oss_variant(
            v: golem_cloud_client::model::TypeVariant,
        ) -> golem_client::model::TypeVariant {
            golem_client::model::TypeVariant {
                cases: v.cases.into_iter().map(to_oss_notp).collect(),
            }
        }

        fn to_oss_result(
            r: golem_cloud_client::model::TypeResult,
        ) -> golem_client::model::TypeResult {
            let golem_cloud_client::model::TypeResult { ok, err } = r;

            golem_client::model::TypeResult {
                ok: ok.map(to_oss_type),
                err: err.map(to_oss_type),
            }
        }

        fn to_oss_option(
            o: golem_cloud_client::model::TypeOption,
        ) -> golem_client::model::TypeOption {
            golem_client::model::TypeOption {
                inner: to_oss_type(o.inner),
            }
        }

        fn to_oss_enum(e: golem_cloud_client::model::TypeEnum) -> golem_client::model::TypeEnum {
            golem_client::model::TypeEnum { cases: e.cases }
        }

        fn to_oss_flags(e: golem_cloud_client::model::TypeFlags) -> golem_client::model::TypeFlags {
            golem_client::model::TypeFlags { cases: e.cases }
        }

        fn to_oss_ntp(
            p: golem_cloud_client::model::NameTypePair,
        ) -> golem_client::model::NameTypePair {
            let golem_cloud_client::model::NameTypePair { name, typ } = p;

            golem_client::model::NameTypePair {
                name,
                typ: to_oss_type(typ),
            }
        }

        fn to_oss_record(
            r: golem_cloud_client::model::TypeRecord,
        ) -> golem_client::model::TypeRecord {
            golem_client::model::TypeRecord {
                cases: r.cases.into_iter().map(to_oss_ntp).collect(),
            }
        }

        fn to_oss_tuple(t: golem_cloud_client::model::TypeTuple) -> golem_client::model::TypeTuple {
            golem_client::model::TypeTuple {
                items: t.items.into_iter().map(to_oss_type).collect(),
            }
        }

        fn to_oss_list(l: golem_cloud_client::model::TypeList) -> golem_client::model::TypeList {
            golem_client::model::TypeList {
                inner: to_oss_type(l.inner),
            }
        }

        fn to_oss_resource_mode(
            m: golem_cloud_client::model::ResourceMode,
        ) -> golem_client::model::ResourceMode {
            match m {
                golem_cloud_client::model::ResourceMode::Borrowed => {
                    golem_client::model::ResourceMode::Borrowed
                }
                golem_cloud_client::model::ResourceMode::Owned => {
                    golem_client::model::ResourceMode::Owned
                }
            }
        }

        fn to_oss_handle(
            h: golem_cloud_client::model::TypeHandle,
        ) -> golem_client::model::TypeHandle {
            golem_client::model::TypeHandle {
                resource_id: h.resource_id,
                mode: to_oss_resource_mode(h.mode),
            }
        }

        fn to_oss_type(t: golem_cloud_client::model::Type) -> golem_client::model::Type {
            match t {
                golem_cloud_client::model::Type::Variant(x) => {
                    golem_client::model::Type::Variant(to_oss_variant(x))
                }
                golem_cloud_client::model::Type::Result(x) => {
                    golem_client::model::Type::Result(Box::new(to_oss_result(*x)))
                }
                golem_cloud_client::model::Type::Option(x) => {
                    golem_client::model::Type::Option(Box::new(to_oss_option(*x)))
                }
                golem_cloud_client::model::Type::Enum(x) => {
                    golem_client::model::Type::Enum(to_oss_enum(x))
                }
                golem_cloud_client::model::Type::Flags(x) => {
                    golem_client::model::Type::Flags(to_oss_flags(x))
                }
                golem_cloud_client::model::Type::Record(x) => {
                    golem_client::model::Type::Record(to_oss_record(x))
                }
                golem_cloud_client::model::Type::Tuple(x) => {
                    golem_client::model::Type::Tuple(to_oss_tuple(x))
                }
                golem_cloud_client::model::Type::List(x) => {
                    golem_client::model::Type::List(Box::new(to_oss_list(*x)))
                }
                golem_cloud_client::model::Type::Str(_) => {
                    golem_client::model::Type::Str(golem_client::model::TypeStr {})
                }
                golem_cloud_client::model::Type::Chr(_) => {
                    golem_client::model::Type::Chr(golem_client::model::TypeChr {})
                }
                golem_cloud_client::model::Type::F64(_) => {
                    golem_client::model::Type::F64(golem_client::model::TypeF64 {})
                }
                golem_cloud_client::model::Type::F32(_) => {
                    golem_client::model::Type::F32(golem_client::model::TypeF32 {})
                }
                golem_cloud_client::model::Type::U64(_) => {
                    golem_client::model::Type::U64(golem_client::model::TypeU64 {})
                }
                golem_cloud_client::model::Type::S64(_) => {
                    golem_client::model::Type::S64(golem_client::model::TypeS64 {})
                }
                golem_cloud_client::model::Type::U32(_) => {
                    golem_client::model::Type::U32(golem_client::model::TypeU32 {})
                }
                golem_cloud_client::model::Type::S32(_) => {
                    golem_client::model::Type::S32(golem_client::model::TypeS32 {})
                }
                golem_cloud_client::model::Type::U16(_) => {
                    golem_client::model::Type::U16(golem_client::model::TypeU16 {})
                }
                golem_cloud_client::model::Type::S16(_) => {
                    golem_client::model::Type::S16(golem_client::model::TypeS16 {})
                }
                golem_cloud_client::model::Type::U8(_) => {
                    golem_client::model::Type::U8(golem_client::model::TypeU8 {})
                }
                golem_cloud_client::model::Type::S8(_) => {
                    golem_client::model::Type::S8(golem_client::model::TypeS8 {})
                }
                golem_cloud_client::model::Type::Bool(_) => {
                    golem_client::model::Type::Bool(golem_client::model::TypeBool {})
                }
                golem_cloud_client::model::Type::Handle(x) => {
                    golem_client::model::Type::Handle(to_oss_handle(x))
                }
            }
        }

        fn to_oss_function_parameter(
            p: golem_cloud_client::model::FunctionParameter,
        ) -> golem_client::model::FunctionParameter {
            let golem_cloud_client::model::FunctionParameter { name, typ } = p;

            golem_client::model::FunctionParameter {
                name,
                typ: to_oss_type(typ),
            }
        }

        fn to_oss_function_result(
            r: golem_cloud_client::model::FunctionResult,
        ) -> golem_client::model::FunctionResult {
            let golem_cloud_client::model::FunctionResult { name, typ } = r;

            golem_client::model::FunctionResult {
                name,
                typ: to_oss_type(typ),
            }
        }

        fn to_oss_export_function(
            f: golem_cloud_client::model::ExportFunction,
        ) -> golem_client::model::ExportFunction {
            let golem_cloud_client::model::ExportFunction {
                name,
                parameters,
                results,
            } = f;

            golem_client::model::ExportFunction {
                name,
                parameters: parameters
                    .into_iter()
                    .map(to_oss_function_parameter)
                    .collect(),
                results: results.into_iter().map(to_oss_function_result).collect(),
            }
        }

        fn to_oss_export_instance(
            i: golem_cloud_client::model::ExportInstance,
        ) -> golem_client::model::ExportInstance {
            let golem_cloud_client::model::ExportInstance { name, functions } = i;

            golem_client::model::ExportInstance {
                name,
                functions: functions.into_iter().map(to_oss_export_function).collect(),
            }
        }

        fn to_oss_export(e: golem_cloud_client::model::Export) -> golem_client::model::Export {
            match e {
                golem_cloud_client::model::Export::Instance(i) => {
                    golem_client::model::Export::Instance(to_oss_export_instance(i))
                }
                golem_cloud_client::model::Export::Function(f) => {
                    golem_client::model::Export::Function(to_oss_export_function(f))
                }
            }
        }

        fn to_oss_versioned_name(
            n: golem_cloud_client::model::VersionedName,
        ) -> golem_client::model::VersionedName {
            let golem_cloud_client::model::VersionedName { name, version } = n;

            golem_client::model::VersionedName { name, version }
        }

        fn to_oss_producer_field(
            f: golem_cloud_client::model::ProducerField,
        ) -> golem_client::model::ProducerField {
            let golem_cloud_client::model::ProducerField { name, values } = f;

            golem_client::model::ProducerField {
                name,
                values: values.into_iter().map(to_oss_versioned_name).collect(),
            }
        }

        fn to_oss_producers(
            p: golem_cloud_client::model::Producers,
        ) -> golem_client::model::Producers {
            golem_client::model::Producers {
                fields: p.fields.into_iter().map(to_oss_producer_field).collect(),
            }
        }

        fn to_oss_memory(
            p: golem_cloud_client::model::LinearMemory,
        ) -> golem_client::model::LinearMemory {
            golem_client::model::LinearMemory {
                initial: p.initial,
                maximum: p.maximum,
            }
        }

        golem_client::model::ComponentMetadata {
            exports: exports.into_iter().map(to_oss_export).collect(),
            producers: producers.into_iter().map(to_oss_producers).collect(),
            memories: memories.into_iter().map(to_oss_memory).collect(),
        }
    }
}

impl ToCli<Component> for golem_cloud_client::model::Component {
    fn to_cli(self) -> Component {
        let golem_cloud_client::model::Component {
            versioned_component_id,
            user_component_id,
            protected_component_id,
            component_name,
            component_size,
            metadata,
            project_id,
        } = self;

        Component {
            versioned_component_id: versioned_component_id.to_oss(),
            user_component_id: user_component_id.to_oss(),
            protected_component_id: protected_component_id.to_oss(),
            component_name,
            component_size,
            metadata: metadata.to_oss(),
            project_id: Some(ProjectId(project_id)),
        }
    }
}
