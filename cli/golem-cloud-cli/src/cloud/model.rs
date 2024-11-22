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

    impl ToOss<golem_client::model::InvokeParameters> for golem_cloud_client::model::InvokeParameters {
        fn to_oss(self) -> golem_client::model::InvokeParameters {
            golem_client::model::InvokeParameters {
                params: self.params,
            }
        }
    }

    impl ToOss<golem_client::model::InvokeResult> for golem_cloud_client::model::InvokeResult {
        fn to_oss(self) -> golem_client::model::InvokeResult {
            golem_client::model::InvokeResult {
                result: self.result,
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

    impl ToCloud<golem_cloud_client::model::InvokeParameters>
        for golem_client::model::InvokeParameters
    {
        fn to_cloud(self) -> golem_cloud_client::model::InvokeParameters {
            golem_cloud_client::model::InvokeParameters {
                params: self.params,
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
                metadata: self.metadata,
                project_id: Some(golem_cli::cloud::ProjectId(self.project_id)),
                created_at: self.created_at,
                component_type: self
                    .component_type
                    .unwrap_or(golem_client::model::ComponentType::Durable),
                files: self.files,
            }
        }
    }
}
