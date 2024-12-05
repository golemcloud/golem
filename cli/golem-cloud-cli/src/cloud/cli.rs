use std::future::Future;
use std::path::PathBuf;

use super::factory::{CloudProfileAuth, CloudServiceFactory};
use super::model::{
    CloudPluginScopeArgs, PluginDefinition, PluginDefinitionWithoutOwner, ProjectAction,
};
use crate::cloud::command::account::AccountSubcommand;
use crate::cloud::command::certificate::CertificateSubcommand;
use crate::cloud::command::domain::DomainSubcommand;
use crate::cloud::command::policy::ProjectPolicySubcommand;
use crate::cloud::command::project::ProjectSubcommand;
use crate::cloud::command::token::TokenSubcommand;
use crate::cloud::model::{CloudComponentUriOrName, ProjectPolicyId, ProjectRef};
use clap::{ArgMatches, Command, Error, FromArgMatches, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use golem_cli::cloud::{AccountId, ProjectId};
use golem_cli::command::profile::UniversalProfileAdd;
use golem_cli::command::worker::WorkerRefSplit;
use golem_cli::command::{CliCommand, NoProfileCommandContext, SharedCommand, StaticSharedCommand};
use golem_cli::config::{CloudProfile, ProfileName};
use golem_cli::factory::ServiceFactory;
use golem_cli::init::ProfileAuth;
use golem_cli::init::{init_profile, CliKind};
use golem_cli::model::{Format, GolemError, GolemResult, WorkerName};
use golem_cli::{check_for_newer_server_version, command, completion};
use golem_cloud_client::model::CloudPluginOwner;
use golem_cloud_client::CloudPluginScope;
use golem_common::model::TargetWorkerId;
use golem_common::uri::cloud::uri::{ComponentUri, ProjectUri, ResourceUri, ToOssUri, WorkerUri};
use golem_common::uri::cloud::url::{ComponentUrl, ProjectUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use std::pin::Pin;
use uuid::Uuid;

pub async fn run<ProfileAdd: clap::Args + Into<UniversalProfileAdd>>(
    config_dir: PathBuf,
    profile_name: ProfileName,
    profile: CloudProfile,
    format: Format,
    command: Command,
    parsed: GolemCloudCli<ProfileAdd>,
) -> Result<GolemResult, GolemError> {
    let config_dir_clone = config_dir.clone();

    // factory needs to be lazy to avoid sending login requests when not needed
    let factory = Box::pin(async move {
        let factory = CloudServiceFactory::from_profile(
            &profile_name,
            &profile,
            &config_dir_clone,
            parsed.auth_token,
        )
        .await?;
        check_for_newer_server_version(factory.version_service().as_ref(), crate::VERSION).await;
        Ok::<CloudServiceFactory, GolemError>(factory)
    });

    let ctx = CloudCommandContext {
        format,
        factory,
        config_dir,
        command,
        cli_kind: CliKind::Cloud,
    };

    parsed.command.run(ctx).await
}

#[derive(clap::Args, Debug, Clone)]
pub struct CloudWorkerNameOrUriArg {
    /// Worker URI. Either URN or URL.
    #[arg(
    short = 'W',
    long,
    conflicts_with_all(["worker_name", "component", "component_name"]),
    required = true,
    value_name = "URI"
    )]
    worker: Option<WorkerUri>,

    /// Component URI. Either URN or URL.
    #[arg(
    short = 'C',
    long,
    conflicts_with_all(["component_name", "worker"]),
    required = true,
    value_name = "URI"
    )]
    component: Option<ComponentUri>,

    /// Project URI. Either URN or URL.
    #[arg(short = 'P', long, conflicts_with_all(["project_name", "component", "worker"]))]
    project: Option<ProjectUri>,

    #[arg(short = 'p', long, conflicts_with_all(["project", "component", "worker"]))]
    project_name: Option<String>,

    #[arg(short, long, conflicts_with_all(["component", "worker"]), required = true)]
    component_name: Option<String>,

    /// Name of the worker
    #[arg(short, long, conflicts_with = "worker", required = true)]
    worker_name: Option<WorkerName>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CloudWorkerUriArg {
    pub uri: WorkerUri,
    pub worker_name: bool,
    pub component_name: bool,
    pub explicit_project: ProjectRef,
}

impl WorkerRefSplit<ProjectRef> for CloudWorkerUriArg {
    fn split(self) -> (golem_common::uri::oss::uri::WorkerUri, Option<ProjectRef>) {
        let CloudWorkerUriArg {
            uri,
            worker_name: _,
            component_name: _,
            explicit_project,
        } = self;

        let (uri, project) = uri.to_oss_uri();

        let project = project.map(ProjectUri::URL).or(explicit_project.uri);

        (
            uri,
            Some(ProjectRef {
                uri: project,
                explicit_name: false,
            }),
        )
    }
}

impl FromArgMatches for CloudWorkerUriArg {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        CloudWorkerNameOrUriArg::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: CloudWorkerNameOrUriArg = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = CloudWorkerNameOrUriArg::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for CloudWorkerUriArg {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        CloudWorkerNameOrUriArg::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        CloudWorkerNameOrUriArg::augment_args_for_update(cmd)
    }
}

impl From<&CloudWorkerNameOrUriArg> for CloudWorkerUriArg {
    fn from(value: &CloudWorkerNameOrUriArg) -> Self {
        match &value.worker {
            Some(uri) => CloudWorkerUriArg {
                uri: uri.clone(),
                worker_name: false,
                component_name: false,
                explicit_project: ProjectRef {
                    uri: None,
                    explicit_name: false,
                },
            },
            None => {
                let worker_name = value.worker_name.clone().unwrap().0;

                match &value.component {
                    Some(ComponentUri::URN(component_urn)) => {
                        let uri = WorkerUri::URN(WorkerUrn {
                            id: TargetWorkerId {
                                component_id: component_urn.id.clone(),
                                worker_name: Some(worker_name),
                            },
                        });
                        CloudWorkerUriArg {
                            uri,
                            worker_name: true,
                            component_name: false,
                            explicit_project: ProjectRef {
                                uri: None,
                                explicit_name: false,
                            },
                        }
                    }
                    Some(ComponentUri::URL(component_url)) => {
                        let uri = WorkerUri::URL(WorkerUrl {
                            component_name: component_url.name.to_string(),
                            worker_name: Some(worker_name),
                            project: component_url.project.clone(),
                        });

                        CloudWorkerUriArg {
                            uri,
                            worker_name: true,
                            component_name: false,
                            explicit_project: ProjectRef {
                                uri: None,
                                explicit_name: false,
                            },
                        }
                    }
                    None => {
                        let component_name = value.component_name.clone().unwrap();

                        let project = match &value.project {
                            Some(p) => ProjectRef {
                                uri: Some(p.clone()),
                                explicit_name: false,
                            },
                            None => match value.project_name.clone() {
                                Some(project_name) => ProjectRef {
                                    uri: Some(ProjectUri::URL(ProjectUrl {
                                        name: project_name,
                                        account: None,
                                    })),
                                    explicit_name: true,
                                },
                                None => ProjectRef {
                                    uri: None,
                                    explicit_name: false,
                                },
                            },
                        };

                        let uri = WorkerUri::URL(WorkerUrl {
                            component_name,
                            worker_name: Some(worker_name),
                            project: None,
                        });

                        CloudWorkerUriArg {
                            uri,
                            worker_name: true,
                            component_name: true,
                            explicit_project: project,
                        }
                    }
                }
            }
        }
    }
}

impl From<&CloudWorkerUriArg> for CloudWorkerNameOrUriArg {
    fn from(value: &CloudWorkerUriArg) -> Self {
        let project_name = match &value.explicit_project.uri {
            Some(ProjectUri::URL(ProjectUrl { name, .. })) => {
                if value.explicit_project.explicit_name {
                    Some(name.clone())
                } else {
                    None
                }
            }
            _ => None,
        };

        let project = match &value.explicit_project.uri {
            Some(uri) => {
                if project_name.is_none() {
                    Some(uri.clone())
                } else {
                    None
                }
            }
            None => None,
        };

        if !value.worker_name {
            CloudWorkerNameOrUriArg {
                worker: Some(value.uri.clone()),
                component: None,
                project,
                project_name,
                component_name: None,
                worker_name: None,
            }
        } else {
            match &value.uri {
                WorkerUri::URN(urn) => {
                    let component_uri = ComponentUri::URN(ComponentUrn {
                        id: urn.id.component_id.clone(),
                    });

                    CloudWorkerNameOrUriArg {
                        worker: None,
                        component: Some(component_uri),
                        project,
                        project_name,
                        component_name: None,
                        worker_name: urn.id.worker_name.as_ref().map(|n| WorkerName(n.clone())),
                    }
                }
                WorkerUri::URL(url) => {
                    if value.component_name {
                        CloudWorkerNameOrUriArg {
                            worker: None,
                            component: None,
                            project,
                            project_name,
                            component_name: Some(url.component_name.to_string()),
                            worker_name: url.worker_name.as_ref().map(|n| WorkerName(n.clone())),
                        }
                    } else {
                        let component_uri = ComponentUri::URL(ComponentUrl {
                            name: url.component_name.to_string(),
                            project: url.project.clone(),
                        });

                        CloudWorkerNameOrUriArg {
                            worker: None,
                            component: Some(component_uri),
                            project,
                            project_name,
                            component_name: None,
                            worker_name: url.worker_name.as_ref().map(|n| WorkerName(n.clone())),
                        }
                    }
                }
            }
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = true, multiple = false)]
pub struct ProjectActionsOrPolicyId {
    /// The sharing policy's identifier. If not provided, use `--project-actions` instead
    #[arg(long, required = true, group = "project_actions_or_policy")]
    pub project_policy_id: Option<ProjectPolicyId>,

    /// A list of actions to be granted to the recipient account. If not provided, use `--project-policy-id` instead
    #[arg(
        short = 'A',
        long,
        required = true,
        group = "project_actions_or_policy"
    )]
    pub project_actions: Option<Vec<ProjectAction>>,
}

/// Shared command with cloud-specific arguments
type SpecializedSharedCommand<ProfileAdd> = SharedCommand<
    ProjectRef,
    CloudComponentUriOrName,
    CloudWorkerUriArg,
    CloudPluginScopeArgs,
    ProfileAdd,
>;

#[derive(Subcommand, Debug)]
#[command()]
pub enum CloudOnlyCommand {
    /// Manage accounts
    #[command()]
    Account {
        /// The account ID to operate on
        #[arg(short = 'A', long, global = true)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: AccountSubcommand,
    },

    /// Manage access tokens
    #[command()]
    Token {
        /// The account ID to operate on
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: TokenSubcommand,
    },

    /// Manage projects
    #[command()]
    Project {
        #[command(subcommand)]
        subcommand: ProjectSubcommand,
    },

    /// Share a project with another account
    #[command()]
    Share {
        /// Project to be shared
        #[command(flatten)]
        project_ref: ProjectRef,

        /// User account the project will be shared with
        #[arg(long)]
        recipient_account_id: AccountId,

        #[command(flatten)]
        project_actions_or_policy_id: ProjectActionsOrPolicyId,
    },

    /// Manage project sharing policies
    #[command()]
    ProjectPolicy {
        #[command(subcommand)]
        subcommand: ProjectPolicySubcommand,
    },

    /// Get resource by URI
    ///
    /// Use resource URN or URL to get resource metadata.
    #[command()]
    Get {
        #[arg(value_name = "URI")]
        uri: ResourceUri,
    },

    /// Manage certificates
    #[command()]
    Certificate {
        #[command(subcommand)]
        subcommand: CertificateSubcommand,
    },

    /// Manage domains
    #[command()]
    Domain {
        #[command(subcommand)]
        subcommand: DomainSubcommand,
    },
}

impl CliCommand<NoProfileCommandContext> for CloudOnlyCommand {
    async fn run(self, ctx: NoProfileCommandContext) -> Result<GolemResult, GolemError> {
        ctx.fail_uninitialized()
    }
}

#[derive(Parser, Debug)]
#[command(author, version = crate::VERSION, about = "Command line interface for Golem Cloud.", long_about = None, rename_all = "kebab-case")]
pub struct GolemCloudCli<ProfileAdd: clap::Args> {
    #[arg(short = 'T', long, global = true)]
    pub auth_token: Option<Uuid>,

    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, global = true)]
    pub format: Option<Format>,

    #[command(subcommand)]
    pub command: command::Zip<
        StaticSharedCommand,
        command::Zip<SpecializedSharedCommand<ProfileAdd>, CloudOnlyCommand>,
    >,
}

pub struct CloudCommandContext {
    format: Format,
    factory: Pin<Box<dyn Future<Output = Result<CloudServiceFactory, GolemError>>>>,
    config_dir: PathBuf,
    command: Command,
    cli_kind: CliKind,
}

impl<ProfileAdd: clap::Args + Into<UniversalProfileAdd>> CliCommand<CloudCommandContext>
    for SpecializedSharedCommand<ProfileAdd>
{
    async fn run(self, ctx: CloudCommandContext) -> Result<GolemResult, GolemError> {
        match self {
            SharedCommand::Component { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(
                        ctx.format,
                        factory.component_service(),
                        factory.deploy_service(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::Worker { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(
                        ctx.format,
                        factory.worker_service(),
                        factory.project_resolver(),
                    )
                    .await
            }
            SharedCommand::ApiDefinition { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(
                        factory.api_definition_service().as_ref(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::ApiDeployment { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(
                        factory.api_deployment_service().as_ref(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::ApiSecurityScheme { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(
                        factory.api_security_scheme_service().as_ref(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::Profile { subcommand } => {
                subcommand
                    .handle(ctx.cli_kind, &ctx.config_dir, &CloudProfileAuth())
                    .await
            }
            SharedCommand::Init {} => {
                let auth = CloudProfileAuth();

                let res = init_profile(
                    ctx.cli_kind,
                    ProfileName::default(ctx.cli_kind),
                    &ctx.config_dir,
                    &auth,
                )
                .await?;

                if res.auth_required {
                    auth.auth(&res.profile_name, &ctx.config_dir).await?
                }

                Ok(GolemResult::Str("Profile created.".to_string()))
            }
            SharedCommand::Completion { generator } => {
                completion::print_completion(ctx.command, generator);
                Ok(GolemResult::Str("".to_string()))
            }
            SharedCommand::Plugin { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle::<PluginDefinition, PluginDefinitionWithoutOwner, ProjectRef, CloudPluginScope, CloudPluginOwner, ProjectId>(
                        ctx.format,
                        factory.plugin_client(),
                        factory.project_resolver(),
                        factory.component_service(),
                    )
                    .await
            }
        }
    }
}

impl CliCommand<CloudCommandContext> for CloudOnlyCommand {
    async fn run(self, ctx: CloudCommandContext) -> Result<GolemResult, GolemError> {
        match self {
            CloudOnlyCommand::Account {
                account_id,
                subcommand,
            } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(
                        account_id,
                        factory.account_service().as_ref(),
                        factory.grant_service().as_ref(),
                    )
                    .await
            }
            CloudOnlyCommand::Token {
                account_id,
                subcommand,
            } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(account_id, factory.token_service().as_ref())
                    .await
            }
            CloudOnlyCommand::Project { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand.handle(factory.project_service().as_ref()).await
            }
            CloudOnlyCommand::Share {
                project_ref,
                recipient_account_id,
                project_actions_or_policy_id,
            } => {
                let factory = ctx.factory.await?;

                factory
                    .project_grant_service()
                    .grant(
                        project_ref,
                        recipient_account_id,
                        project_actions_or_policy_id.project_policy_id,
                        project_actions_or_policy_id.project_actions,
                    )
                    .await
            }
            CloudOnlyCommand::ProjectPolicy { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(factory.project_policy_service().as_ref())
                    .await
            }
            CloudOnlyCommand::Certificate { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand
                    .handle(factory.certificate_service().as_ref())
                    .await
            }
            CloudOnlyCommand::Domain { subcommand } => {
                let factory = ctx.factory.await?;

                subcommand.handle(factory.domain_service().as_ref()).await
            }
            CloudOnlyCommand::Get { uri } => {
                let factory = ctx.factory.await?;

                crate::cloud::resource::get_resource_by_uri(uri, &factory).await
            }
        }
    }
}
