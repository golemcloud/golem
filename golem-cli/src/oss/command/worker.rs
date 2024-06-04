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

use clap::builder::ValueParser;
use clap::Subcommand;
use golem_client::model::ScanCursor;

use crate::model::{
    ComponentIdOrName, Format, GolemError, GolemResult, IdempotencyKey, JsonValueParser,
    WorkerName, WorkerUpdateMode,
};
use crate::oss::model::OssContext;
use crate::parse_key_val;
use crate::service::worker::WorkerService;

#[derive(Subcommand, Debug)]
#[command()]
pub enum WorkerSubcommand {
    /// Creates a new idle worker
    #[command()]
    Add {
        /// The Golem component to use for the worker, identified by either its name or its component ID
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

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
        /// The Golem component the worker to be invoked belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,

        /// A pre-generated idempotency key
        #[arg(short = 'k', long)]
        idempotency_key: Option<IdempotencyKey>,

        /// Name of the function to be invoked
        #[arg(short, long)]
        function: String,

        /// JSON array representing the parameters to be passed to the function
        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser), conflicts_with = "wave")]
        parameters: Option<serde_json::value::Value>,

        /// Function parameter in WAVE format
        ///
        /// You can specify this argument multiple times for multiple parameters.
        #[arg(
            short = 'p',
            long = "param",
            value_name = "wave",
            conflicts_with = "parameters"
        )]
        wave: Vec<String>,

        /// Enables the STDIO cal;ing convention, passing the parameters through stdin instead of a typed exported interface
        #[arg(short = 's', long, default_value_t = false)]
        use_stdio: bool,
    },

    /// Triggers a function invocation on a worker without waiting for its completion
    #[command()]
    Invoke {
        /// The Golem component the worker to be invoked belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,

        /// A pre-generated idempotency key
        #[arg(short = 'k', long)]
        idempotency_key: Option<IdempotencyKey>,

        /// Name of the function to be invoked
        #[arg(short, long)]
        function: String,

        /// JSON array representing the parameters to be passed to the function
        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser), conflicts_with = "wave")]
        parameters: Option<serde_json::value::Value>,

        /// Function parameter in WAVE format
        ///
        /// You can specify this argument multiple times for multiple parameters.
        #[arg(
            short = 'p',
            long = "param",
            value_name = "wave",
            conflicts_with = "parameters"
        )]
        wave: Vec<String>,
    },

    /// Connect to a worker and live stream its standard output, error and log channels
    #[command()]
    Connect {
        /// The Golem component the worker to be connected to belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Interrupts a running worker
    #[command()]
    Interrupt {
        /// The Golem component the worker to be interrupted belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Simulates a crash on a worker for testing purposes.
    ///
    /// The worker starts recovering and resuming immediately.
    #[command()]
    SimulatedCrash {
        /// The Golem component the worker to be crashed belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Deletes a worker
    #[command()]
    Delete {
        /// The Golem component the worker to be deleted belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Retrieves metadata about an existing worker
    #[command()]
    Get {
        /// The Golem component the worker to be retrieved belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },
    /// Retrieves metadata about an existing workers in a component
    #[command()]
    List {
        /// The Golem component the workers to be retrieved belongs to
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

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
        #[arg(short = 'P', long, value_parser = parse_cursor)]
        cursor: Option<ScanCursor>,

        /// Count of listed values, if count is not provided, returns all values
        #[arg(short = 'n', long)]
        count: Option<u64>,

        /// Precision in relation to worker status, if true, calculate the most up-to-date status for each worker, default is false
        #[arg(short, long)]
        precise: Option<bool>,
    },
    /// Updates a worker
    #[command()]
    Update {
        /// The Golem component of the worker, identified by either its name or its component ID
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        /// Name of the worker to update
        #[arg(short, long)]
        worker_name: WorkerName,

        /// Update mode - auto or manual
        #[arg(short, long)]
        mode: WorkerUpdateMode,

        /// The new version of the updated worker
        #[arg(short = 't', long)]
        target_version: u64,
    },
}

impl WorkerSubcommand {
    pub async fn handle(
        self,
        format: Format,
        service: &(dyn WorkerService<ProjectContext = OssContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            WorkerSubcommand::Add {
                component_id_or_name,
                worker_name,
                env,
                args,
            } => {
                service
                    .add(component_id_or_name, worker_name, env, args, None)
                    .await
            }
            WorkerSubcommand::IdempotencyKey {} => service.idempotency_key().await,
            WorkerSubcommand::InvokeAndAwait {
                component_id_or_name,
                worker_name,
                idempotency_key,
                function,
                parameters,
                wave,
                use_stdio,
            } => {
                service
                    .invoke_and_await(
                        format,
                        component_id_or_name,
                        worker_name,
                        idempotency_key,
                        function,
                        parameters,
                        wave,
                        use_stdio,
                        None,
                    )
                    .await
            }
            WorkerSubcommand::Invoke {
                component_id_or_name,
                worker_name,
                idempotency_key,
                function,
                parameters,
                wave,
            } => {
                service
                    .invoke(
                        component_id_or_name,
                        worker_name,
                        idempotency_key,
                        function,
                        parameters,
                        wave,
                        None,
                    )
                    .await
            }
            WorkerSubcommand::Connect {
                component_id_or_name,
                worker_name,
            } => {
                service
                    .connect(component_id_or_name, worker_name, None)
                    .await
            }
            WorkerSubcommand::Interrupt {
                component_id_or_name,
                worker_name,
            } => {
                service
                    .interrupt(component_id_or_name, worker_name, None)
                    .await
            }
            WorkerSubcommand::SimulatedCrash {
                component_id_or_name,
                worker_name,
            } => {
                service
                    .simulated_crash(component_id_or_name, worker_name, None)
                    .await
            }
            WorkerSubcommand::Delete {
                component_id_or_name,
                worker_name,
            } => {
                service
                    .delete(component_id_or_name, worker_name, None)
                    .await
            }
            WorkerSubcommand::Get {
                component_id_or_name,
                worker_name,
            } => service.get(component_id_or_name, worker_name, None).await,
            WorkerSubcommand::List {
                component_id_or_name,
                filter,
                count,
                cursor,
                precise,
            } => {
                service
                    .list(component_id_or_name, filter, count, cursor, precise, None)
                    .await
            }
            WorkerSubcommand::Update {
                component_id_or_name,
                worker_name,
                target_version,
                mode,
            } => {
                service
                    .update(
                        component_id_or_name,
                        worker_name,
                        target_version,
                        mode,
                        None,
                    )
                    .await
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
