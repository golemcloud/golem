// Copyright 2024-2025 Golem Cloud
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

use crate::command::ComponentRefSplit;
use clap::builder::ValueParser;
use clap::{ArgMatches, Args, Error, FromArgMatches, Subcommand};
use golem_client::model::ScanCursor;
use golem_common::model::TargetWorkerId;
use golem_common::uri::oss::uri::{ComponentUri, WorkerUri};
use golem_common::uri::oss::url::{ComponentUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use std::sync::Arc;
use tokio::join;
use tokio::task::spawn;

use crate::model::{
    Format, GolemError, GolemResult, IdempotencyKey, JsonValueParser, WorkerName, WorkerUpdateMode,
};
use crate::oss::model::OssContext;
use crate::service::project::ProjectResolver;
use crate::service::worker::WorkerService;
use crate::{parse_bool, parse_key_val};

#[derive(clap::Args, Debug, Clone)]
pub struct OssWorkerNameOrUriArg {
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

    #[arg(short, long, conflicts_with_all(["component", "worker"]), required = true)]
    component_name: Option<String>,

    /// Name of the worker
    #[arg(short, long, conflicts_with = "worker", required = true)]
    worker_name: Option<WorkerName>,
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct InvokeParameterList {
    /// JSON array representing the parameters to be passed to the function
    #[arg(
        short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser), group = "param"
    )]
    parameters: Option<serde_json::value::Value>,

    /// Function parameter in WAVE format
    ///
    /// You can specify this argument multiple times for multiple parameters.
    #[arg(short = 'a', long = "arg", value_name = "wave", group = "param")]
    wave: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OssWorkerUriArg {
    pub uri: WorkerUri,
    pub worker_name: bool,
    pub component_name: bool,
}

impl FromArgMatches for OssWorkerUriArg {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        OssWorkerNameOrUriArg::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: OssWorkerNameOrUriArg = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = OssWorkerNameOrUriArg::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for OssWorkerUriArg {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        OssWorkerNameOrUriArg::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        OssWorkerNameOrUriArg::augment_args_for_update(cmd)
    }
}

impl From<&OssWorkerNameOrUriArg> for OssWorkerUriArg {
    fn from(value: &OssWorkerNameOrUriArg) -> Self {
        match &value.worker {
            Some(uri) => OssWorkerUriArg {
                uri: uri.clone(),
                worker_name: false,
                component_name: false,
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
                        OssWorkerUriArg {
                            uri,
                            worker_name: true,
                            component_name: false,
                        }
                    }
                    Some(ComponentUri::URL(component_url)) => {
                        let uri = WorkerUri::URL(WorkerUrl {
                            component_name: component_url.name.to_string(),
                            worker_name: Some(worker_name),
                        });

                        OssWorkerUriArg {
                            uri,
                            worker_name: true,
                            component_name: false,
                        }
                    }
                    None => {
                        let component_name = value.component_name.clone().unwrap();
                        let uri = WorkerUri::URL(WorkerUrl {
                            component_name,
                            worker_name: Some(worker_name),
                        });

                        OssWorkerUriArg {
                            uri,
                            worker_name: true,
                            component_name: true,
                        }
                    }
                }
            }
        }
    }
}

impl From<&OssWorkerUriArg> for OssWorkerNameOrUriArg {
    fn from(value: &OssWorkerUriArg) -> Self {
        if !value.worker_name {
            OssWorkerNameOrUriArg {
                worker: Some(value.uri.clone()),
                component: None,
                component_name: None,
                worker_name: None,
            }
        } else {
            match &value.uri {
                WorkerUri::URN(urn) => {
                    let component_uri = ComponentUri::URN(ComponentUrn {
                        id: urn.id.component_id.clone(),
                    });

                    OssWorkerNameOrUriArg {
                        worker: None,
                        component: Some(component_uri),
                        component_name: None,
                        worker_name: urn
                            .id
                            .worker_name
                            .as_ref()
                            .map(|n| WorkerName(n.to_string())),
                    }
                }
                WorkerUri::URL(url) => {
                    if value.component_name {
                        OssWorkerNameOrUriArg {
                            worker: None,
                            component: None,
                            component_name: Some(url.component_name.to_string()),
                            worker_name: url
                                .worker_name
                                .as_ref()
                                .map(|n| WorkerName(n.to_string())),
                        }
                    } else {
                        let component_uri = ComponentUri::URL(ComponentUrl {
                            name: url.component_name.to_string(),
                        });

                        OssWorkerNameOrUriArg {
                            worker: None,
                            component: Some(component_uri),
                            component_name: None,
                            worker_name: url
                                .worker_name
                                .as_ref()
                                .map(|n| WorkerName(n.to_string())),
                        }
                    }
                }
            }
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct WorkerConnectOptions {
    /// Use colored log lines in text mode
    #[arg(long,
          action=clap::ArgAction::Set,
          value_parser = parse_bool,
          default_missing_value="true",
          default_value_t=true,
          num_args = 0..=1,
          require_equals = false,
    )]
    pub colors: bool,

    /// Show a timestamp for each log line
    #[arg(long,
          action=clap::ArgAction::Set,
          value_parser = parse_bool,
          default_missing_value="true",
          default_value_t=true,
          num_args = 0..=1,
          require_equals = false,
    )]
    pub show_timestamp: bool,

    /// Show the source or log level for each log line
    #[arg(long,
          action=clap::ArgAction::Set,
          value_parser = parse_bool,
          default_missing_value="true",
          default_value_t=false,
          num_args = 0..=1,
          require_equals = false,
    )]
    pub show_level: bool,
}

#[derive(Subcommand, Debug)]
#[command()]
pub enum WorkerSubcommand<ComponentRef: clap::Args, WorkerRef: clap::Args> {
    /// Creates a new idle worker
    #[command(alias = "start", alias = "create")]
    Add {
        /// The Golem component to use for the worker
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Name of the newly created worker
        #[arg(short, long)]
        worker_name: WorkerName,

        /// List of environment variables (key-value pairs) passed to the worker
        #[arg(short, long, value_parser = parse_key_val, value_name = "ENV=VAL")]
        env: Vec<(String, String)>,

        /// List of command line arguments passed to the worker
        #[arg(value_name = "args")]
        args: Vec<String>,
    },

    /// Generates an idempotency key for achieving at-most-one invocation when doing retries
    #[command()]
    IdempotencyKey {},

    /// Invokes a worker and waits for its completion
    #[command()]
    InvokeAndAwait {
        #[command(flatten)]
        worker_ref: WorkerRef,

        /// A pre-generated idempotency key
        #[arg(short = 'k', long)]
        idempotency_key: Option<IdempotencyKey>,

        /// Name of the function to be invoked
        #[arg(short, long)]
        function: String,

        #[command(flatten)]
        parameters: InvokeParameterList,

        /// Connect to the worker during the invocation and show its logs
        #[arg(long)]
        connect: bool,

        #[command(flatten)]
        connect_options: WorkerConnectOptions,
    },

    /// Triggers a function invocation on a worker without waiting for its completion
    #[command()]
    Invoke {
        #[command(flatten)]
        worker_ref: WorkerRef,

        /// A pre-generated idempotency key
        #[arg(short = 'k', long)]
        idempotency_key: Option<IdempotencyKey>,

        /// Name of the function to be invoked
        #[arg(short, long)]
        function: String,

        #[command(flatten)]
        parameters: InvokeParameterList,

        /// Connect to the worker and show its logs
        #[arg(long)]
        connect: bool,

        #[command(flatten)]
        connect_options: WorkerConnectOptions,
    },

    /// Connect to a worker and live stream its standard output, error and log channels
    #[command()]
    Connect {
        #[command(flatten)]
        worker_ref: WorkerRef,
        #[command(flatten)]
        connect_options: WorkerConnectOptions,
    },

    /// Interrupts a running worker
    #[command()]
    Interrupt {
        #[command(flatten)]
        worker_ref: WorkerRef,
    },

    /// Resume an interrupted worker
    #[command()]
    Resume {
        #[command(flatten)]
        worker_ref: WorkerRef,
    },

    /// Simulates a crash on a worker for testing purposes.
    ///
    /// The worker starts recovering and resuming immediately.
    #[command()]
    SimulatedCrash {
        #[command(flatten)]
        worker_ref: WorkerRef,
    },

    /// Deletes a worker
    #[command()]
    Delete {
        #[command(flatten)]
        worker_ref: WorkerRef,
    },

    /// Retrieves metadata about an existing worker
    #[command()]
    Get {
        #[command(flatten)]
        worker_ref: WorkerRef,
    },
    /// Retrieves metadata about an existing workers in a component
    #[command()]
    List {
        /// The Golem component the workers to be retrieved belongs to
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Filter for worker metadata in form of `property op value`.
        ///
        /// Filter examples: `name = worker-name`, `version >= 0`, `status = Running`, `env.var1 = value`.
        /// Can be used multiple times (AND condition is applied between them)
        #[arg(short, long)]
        filter: Option<Vec<String>>,

        /// Position where to start listing, if not provided, starts from the beginning
        ///
        /// It is used to get the next page of results. To get next page, use the cursor returned in the response.
        /// The cursor has the format 'layer/position' where both layer and position are numbers.
        #[arg(short = 'S', long, value_parser = parse_cursor)]
        cursor: Option<ScanCursor>,

        /// Count of listed values, if count is not provided, returns all values
        #[arg(short = 'n', long)]
        count: Option<u64>,

        /// Precision in relation to worker status, if true, calculate the most up-to-date status for each worker, default is false
        #[arg(long)]
        precise: Option<bool>,
    },
    /// Updates a worker
    #[command()]
    Update {
        #[command(flatten)]
        worker_ref: WorkerRef,

        /// Update mode - auto or manual
        #[arg(short, long)]
        mode: WorkerUpdateMode,

        /// The new version of the updated worker
        #[arg(short = 't', long)]
        target_version: u64,
    },
    /// Updates a set of workers
    #[command()]
    UpdateMany {
        /// The Golem component the workers to be retrieved belongs to
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Filter for selecting workers by their metadata in form of `property op value`.
        ///
        /// Filter examples: `name = worker-name`, `version >= 0`, `status = Running`, `env.var1 = value`.
        /// Can be used multiple times (AND condition is applied between them)
        #[arg(short, long)]
        filter: Option<Vec<String>>,

        /// Update mode - auto or manual
        #[arg(short, long)]
        mode: WorkerUpdateMode,

        /// The new version of the updated workers
        #[arg(short = 't', long)]
        target_version: u64,
    },
    /// Queries and dumps a worker's full oplog
    #[command()]
    Oplog {
        #[command(flatten)]
        worker_ref: WorkerRef,

        /// Index of the first oplog entry to get. If missing, the whole oplog is returned
        #[arg(short, long, conflicts_with = "query")]
        from: Option<u64>,

        /// Lucene query to look for oplog entries. If missing, the whole oplog is returned
        #[arg(long, conflicts_with = "from")]
        query: Option<String>,
    },
}

pub trait WorkerRefSplit<ProjectRef> {
    fn split(self) -> (WorkerUri, Option<ProjectRef>);
}

impl WorkerRefSplit<OssContext> for OssWorkerUriArg {
    fn split(self) -> (WorkerUri, Option<OssContext>) {
        (self.uri, None)
    }
}

impl<ComponentRef: clap::Args, WorkerRef: clap::Args> WorkerSubcommand<ComponentRef, WorkerRef> {
    pub async fn handle<
        ProjectRef: Send + Sync + 'static,
        ProjectContext: Clone + Send + Sync + 'static,
    >(
        self,
        format: Format,
        service: Arc<dyn WorkerService<ProjectContext = ProjectContext> + Send + Sync>,
        projects: Arc<dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync>,
    ) -> Result<GolemResult, GolemError>
    where
        ComponentRef: ComponentRefSplit<ProjectRef>,
        WorkerRef: WorkerRefSplit<ProjectRef>,
    {
        match self {
            WorkerSubcommand::Add {
                component_name_or_uri,
                worker_name,
                env,
                args,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .add(component_name_or_uri, worker_name, env, args, project_id)
                    .await
            }
            WorkerSubcommand::IdempotencyKey {} => service.idempotency_key().await,
            WorkerSubcommand::InvokeAndAwait {
                worker_ref,
                idempotency_key,
                function,
                parameters,
                connect,
                connect_options,
            } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                if connect {
                    let worker_uri_clone = worker_uri.clone();
                    let project_id_clone = project_id.clone();
                    let service_clone = service.clone();
                    let connect_handle = spawn(async move {
                        let _ = service_clone
                            .connect(worker_uri_clone, project_id_clone, connect_options, format)
                            .await;
                    });
                    let result = service
                        .invoke_and_await(
                            format,
                            worker_uri,
                            idempotency_key,
                            function,
                            parameters.parameters,
                            parameters.wave,
                            project_id,
                        )
                        .await;

                    connect_handle.abort();
                    result
                } else {
                    service
                        .invoke_and_await(
                            format,
                            worker_uri,
                            idempotency_key,
                            function,
                            parameters.parameters,
                            parameters.wave,
                            project_id,
                        )
                        .await
                }
            }
            WorkerSubcommand::Invoke {
                worker_ref,
                idempotency_key,
                function,
                parameters,
                connect,
                connect_options,
            } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;

                if connect {
                    let invoke_future = service.invoke(
                        worker_uri.clone(),
                        idempotency_key,
                        function,
                        parameters.parameters,
                        parameters.wave,
                        project_id.clone(),
                    );
                    let connect_future =
                        service.connect(worker_uri, project_id, connect_options, format);

                    join!(invoke_future, connect_future).0
                } else {
                    service
                        .invoke(
                            worker_uri,
                            idempotency_key,
                            function,
                            parameters.parameters,
                            parameters.wave,
                            project_id,
                        )
                        .await
                }
            }
            WorkerSubcommand::Connect {
                worker_ref,
                connect_options,
            } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .connect(worker_uri, project_id, connect_options, format)
                    .await
            }
            WorkerSubcommand::Interrupt { worker_ref } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service.interrupt(worker_uri, project_id).await
            }
            WorkerSubcommand::Resume { worker_ref } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service.resume(worker_uri, project_id).await
            }
            WorkerSubcommand::SimulatedCrash { worker_ref } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service.simulated_crash(worker_uri, project_id).await
            }
            WorkerSubcommand::Delete { worker_ref } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service.delete(worker_uri, project_id).await
            }
            WorkerSubcommand::Get { worker_ref } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service.get(worker_uri, project_id).await
            }
            WorkerSubcommand::List {
                component_name_or_uri,
                filter,
                count,
                cursor,
                precise,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .list(
                        component_name_or_uri,
                        filter,
                        count,
                        cursor,
                        precise,
                        project_id,
                    )
                    .await
            }
            WorkerSubcommand::Update {
                worker_ref,
                target_version,
                mode,
            } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .update(worker_uri, target_version, mode, project_id)
                    .await
            }
            WorkerSubcommand::UpdateMany {
                component_name_or_uri,
                filter,
                mode,
                target_version,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .update_many(
                        component_name_or_uri,
                        filter,
                        target_version,
                        mode,
                        project_id,
                    )
                    .await
            }
            WorkerSubcommand::Oplog {
                worker_ref,
                from,
                query,
            } => {
                let (worker_uri, project_ref) = worker_ref.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                match (from, query) {
                    (Some(_), Some(_)) => Err(GolemError(
                        "Only one of 'from' and 'query' can be specified".to_string(),
                    )),
                    (None, None) => service.get_oplog(worker_uri, 0, project_id).await,
                    (None, Some(query)) => {
                        service.search_oplog(worker_uri, query, project_id).await
                    }
                    (Some(from), None) => service.get_oplog(worker_uri, from, project_id).await,
                }
            }
        }
    }
}

fn parse_cursor(s: &str) -> Result<ScanCursor, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let parts = s.split('/').collect::<Vec<_>>();

    if parts.len() != 2 {
        return Err(format!("Invalid cursor format: {}", s).into());
    }

    Ok(ScanCursor {
        layer: parts[0].parse()?,
        cursor: parts[1].parse()?,
    })
}
