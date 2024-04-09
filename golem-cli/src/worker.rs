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

use async_trait::async_trait;
use clap::builder::ValueParser;
use clap::Subcommand;
use golem_client::model::InvokeParameters;

use crate::clients::worker::WorkerClient;
use crate::model::{
    GolemError, GolemResult, InvocationKey, JsonValueParser, TemplateIdOrName, WorkerName,
};
use crate::parse_key_val;
use crate::template::TemplateHandler;

#[derive(Subcommand, Debug)]
#[command()]
pub enum WorkerSubcommand {
    /// Creates a new idle worker
    #[command()]
    Add {
        /// The Golem template to use for the worker, identified by either its name or its template ID
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

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

    /// Generates an invocation ID for achieving at-most-one invocation when doing retries
    #[command()]
    InvocationKey {
        /// The Golem template the worker to be invoked belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Invokes a worker and waits for its completion
    #[command()]
    InvokeAndAwait {
        /// The Golem template the worker to be invoked belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,

        /// A pre-generated invocation key, if not provided, a new one will be generated
        #[arg(short = 'k', long)]
        invocation_key: Option<InvocationKey>,

        /// Name of the function to be invoked
        #[arg(short, long)]
        function: String,

        /// JSON array representing the parameters to be passed to the function
        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser))]
        parameters: serde_json::value::Value,

        /// Enables the STDIO cal;ing convention, passing the parameters through stdin instead of a typed exported interface
        #[arg(short = 's', long, default_value_t = false)]
        use_stdio: bool,
    },

    /// Triggers a function invocation on a worker without waiting for its completion
    #[command()]
    Invoke {
        /// The Golem template the worker to be invoked belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,

        /// Name of the function to be invoked
        #[arg(short, long)]
        function: String,

        /// JSON array representing the parameters to be passed to the function
        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser))]
        parameters: serde_json::value::Value,
    },

    /// Connect to a worker and live stream its standard output, error and log channels
    #[command()]
    Connect {
        /// The Golem template the worker to be connected to belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Interrupts a running worker
    #[command()]
    Interrupt {
        /// The Golem template the worker to be interrupted belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Simulates a crash on a worker for testing purposes.
    ///
    /// The worker starts recovering and resuming immediately.
    #[command()]
    SimulatedCrash {
        /// The Golem template the worker to be crashed belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Deletes a worker
    #[command()]
    Delete {
        /// The Golem template the worker to be deleted belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },

    /// Retrieves metadata about an existing worker
    #[command()]
    Get {
        /// The Golem template the worker to be retrieved belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        /// Name of the worker
        #[arg(short, long)]
        worker_name: WorkerName,
    },
    /// Retrieves metadata about an existing workers in a template
    #[command()]
    List {
        /// The Golem template the workers to be retrieved belongs to
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        // Filter
        #[arg(short, long)]
        filter: Option<Vec<String>>,

        // Cursor
        #[arg(short, long)]
        cursor: Option<u64>,

        // Count
        #[arg(short = 's', long)]
        count: Option<u64>,

        // Precise
        #[arg(short, long)]
        precise: Option<bool>,
    },
}

#[async_trait]
pub trait WorkerHandler {
    async fn handle(&self, subcommand: WorkerSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct WorkerHandlerLive<'r, C: WorkerClient + Send + Sync, R: TemplateHandler + Send + Sync> {
    pub client: C,
    pub templates: &'r R,
}

#[async_trait]
impl<'r, C: WorkerClient + Send + Sync, R: TemplateHandler + Send + Sync> WorkerHandler
    for WorkerHandlerLive<'r, C, R>
{
    async fn handle(&self, subcommand: WorkerSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            WorkerSubcommand::Add {
                template_id_or_name,
                worker_name,
                env,
                args,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let inst = self
                    .client
                    .new_worker(worker_name, template_id, args, env)
                    .await?;

                Ok(GolemResult::Ok(Box::new(inst)))
            }
            WorkerSubcommand::InvocationKey {
                template_id_or_name,
                worker_name,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let key = self
                    .client
                    .get_invocation_key(&worker_name, &template_id)
                    .await?;

                Ok(GolemResult::Ok(Box::new(key)))
            }
            WorkerSubcommand::InvokeAndAwait {
                template_id_or_name,
                worker_name,
                invocation_key,
                function,
                parameters,
                use_stdio,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let invocation_key = match invocation_key {
                    None => {
                        self.client
                            .get_invocation_key(&worker_name, &template_id)
                            .await?
                    }
                    Some(key) => key,
                };

                let res = self
                    .client
                    .invoke_and_await(
                        worker_name,
                        template_id,
                        function,
                        InvokeParameters { params: parameters },
                        invocation_key,
                        use_stdio,
                    )
                    .await?;

                Ok(GolemResult::Json(res.result))
            }
            WorkerSubcommand::Invoke {
                template_id_or_name,
                worker_name,
                function,
                parameters,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                self.client
                    .invoke(
                        worker_name,
                        template_id,
                        function,
                        InvokeParameters { params: parameters },
                    )
                    .await?;

                Ok(GolemResult::Str("Invoked".to_string()))
            }
            WorkerSubcommand::Connect {
                template_id_or_name,
                worker_name,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let result = self.client.connect(worker_name, template_id).await;

                match result {
                    Ok(_) => Err(GolemError("Unexpected connection closure".to_string())),
                    Err(err) => Err(GolemError(err.to_string())),
                }
            }
            WorkerSubcommand::Interrupt {
                template_id_or_name,
                worker_name,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                self.client.interrupt(worker_name, template_id).await?;

                Ok(GolemResult::Str("Interrupted".to_string()))
            }
            WorkerSubcommand::SimulatedCrash {
                template_id_or_name,
                worker_name,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                self.client
                    .simulated_crash(worker_name, template_id)
                    .await?;

                Ok(GolemResult::Str("Done".to_string()))
            }
            WorkerSubcommand::Delete {
                template_id_or_name,
                worker_name,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                self.client.delete(worker_name, template_id).await?;

                Ok(GolemResult::Str("Deleted".to_string()))
            }
            WorkerSubcommand::Get {
                template_id_or_name,
                worker_name,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let response = self.client.get_metadata(worker_name, template_id).await?;

                Ok(GolemResult::Ok(Box::new(response)))
            }
            WorkerSubcommand::List {
                template_id_or_name,
                filter,
                count,
                cursor,
                precise,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let response = self
                    .client
                    .list_metadata(template_id, filter, cursor, count, precise)
                    .await?;

                Ok(GolemResult::Ok(Box::new(response)))
            }
        }
    }
}
