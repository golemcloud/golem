use crate::cloud::command::account::AccountSubcommand;
use crate::cloud::command::certificate::CertificateSubcommand;
use crate::cloud::command::domain::DomainSubcommand;
use crate::cloud::command::policy::ProjectPolicySubcommand;
use crate::cloud::command::project::ProjectSubcommand;
use crate::cloud::command::token::TokenSubcommand;
use crate::cloud::model::{CloudComponentUriOrName, ProjectAction, ProjectPolicyId, ProjectRef};
use clap::{ArgMatches, Error, FromArgMatches, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use golem_cli::cloud::AccountId;
use golem_cli::command::api_definition::ApiDefinitionSubcommand;
use golem_cli::command::api_deployment::ApiDeploymentSubcommand;
use golem_cli::command::component::ComponentSubCommand;
use golem_cli::command::profile::ProfileSubCommand;
use golem_cli::command::worker::{WorkerRefSplit, WorkerSubcommand};
use golem_cli::diagnose;
use golem_cli::model::{Format, WorkerName};
use golem_common::model::WorkerId;
use golem_common::uri::cloud::uri::{ComponentUri, ProjectUri, ResourceUri, ToOssUri, WorkerUri};
use golem_common::uri::cloud::url::{ComponentUrl, ProjectUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use uuid::Uuid;

pub mod account;
pub mod certificate;
pub mod domain;
pub mod policy;
pub mod project;
pub mod token;

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
                            id: WorkerId {
                                component_id: component_urn.id.clone(),
                                worker_name,
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
                            worker_name,
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
                            worker_name,
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
                        worker_name: Some(WorkerName(urn.id.worker_name.to_string())),
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
                            worker_name: Some(WorkerName(url.worker_name.to_string())),
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
                            worker_name: Some(WorkerName(url.worker_name.to_string())),
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

#[derive(Subcommand, Debug)]
#[command()]
pub enum CloudCommand<ProfileAdd: clap::Args> {
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand<ProjectRef, CloudComponentUriOrName>,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand<CloudComponentUriOrName, CloudWorkerUriArg>,
    },

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

    /// Create a new Golem component from built-in examples
    #[command(flatten)]
    Examples(golem_examples::cli::Command),

    /// WASM RPC stub generator
    #[cfg(feature = "stubgen")]
    Stubgen {
        #[command(subcommand)]
        subcommand: golem_wasm_rpc_stubgen::Command,
    },

    /// Manage Golem api definitions
    #[command()]
    ApiDefinition {
        #[command(subcommand)]
        subcommand: ApiDefinitionSubcommand<ProjectRef>,
    },

    /// Manage Golem api deployments
    #[command()]
    ApiDeployment {
        #[command(subcommand)]
        subcommand: ApiDeploymentSubcommand<ProjectRef>,
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

    /// Manage profiles
    #[command()]
    Profile {
        #[command(subcommand)]
        subcommand: ProfileSubCommand<ProfileAdd>,
    },

    /// Interactively creates default profile
    #[command()]
    Init {},

    /// Generate shell completions
    #[command()]
    Completion {
        #[arg(long = "generate", value_enum)]
        generator: clap_complete::Shell,
    },

    /// Diagnose required tooling
    #[command()]
    Diagnose {
        #[command(flatten)]
        command: diagnose::cli::Command,
    },
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Command line interface for Golem Cloud.", long_about = None, rename_all = "kebab-case")]
pub struct GolemCloudCommand<ProfileAdd: clap::Args> {
    #[arg(short = 'T', long, global = true)]
    pub auth_token: Option<Uuid>,

    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, global = true)]
    pub format: Option<Format>,

    #[command(subcommand)]
    pub command: CloudCommand<ProfileAdd>,
}
