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
use golem_client::model::{
    InvokeParameters, InvokeResult, Template, WorkerMetadata, WorkersMetadataResponse,
};
use tokio::task::JoinHandle;
use golem_client::Context;
use crate::clients::template::TemplateClientLive;

use crate::clients::worker::{WorkerClient, WorkerClientLive};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::{
    GolemError, GolemResult, InvocationKey, JsonValueParser, TemplateId, TemplateIdOrName,
    WorkerName,
};
use crate::parse_key_val;
use crate::template::{TemplateHandler, TemplateHandlerLive};

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

        /// Human-readable response format.
        ///
        /// Temporary flag. Should be replaced with the new --format option.
        #[arg(short = 'H', long, default_value_t = false)]
        human_readable: bool,
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

        /// Filter for worker metadata in form of `property op value`.
        ///
        /// Filter examples: `name = worker-name`, `version >= 0`, `status = Running`, `env.var1 = value`.
        /// Can be used multiple times (AND condition is applied between them)
        #[arg(short, long)]
        filter: Option<Vec<String>>,

        /// Position where to start listing, if not provided, starts from the beginning
        ///
        /// It is used to get the next page of results. To get next page, use the cursor returned in the response
        #[arg(short, long)]
        cursor: Option<u64>,

        /// Count of listed values, if count is not provided, returns all values
        #[arg(short = 'n', long)]
        count: Option<u64>,

        /// Precision in relation to worker status, if true, calculate the most up-to-date status for each worker, default is false
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
    pub worker_context: Context,
    pub template_context: Context,
    pub allow_insecure: bool,
}

// same as resolve_worker_template_version, but with no borrowing, so we can spawn it.
async fn resolve_worker_template_version_no_ref(
    worker_context: Context,
    template_context: Context,
    allow_insecure: bool,
    template_id: TemplateId,
    worker_name: WorkerName,
) -> Result<Template, GolemError> {
    let client = WorkerClientLive {
        client: golem_client::api::WorkerClientLive {
            context: worker_context.clone(),
        },
        context: worker_context.clone(),
        allow_insecure,
    };

    let templates = TemplateHandlerLive {
        client: TemplateClientLive {
            client: golem_client::api::TemplateClientLive {
                context: template_context.clone(),
            },
        },
    };
    resolve_worker_template_version(&client, &templates, template_id, worker_name).await
}

async fn resolve_worker_template_version<
    C: WorkerClient + Send + Sync,
    R: TemplateHandler + Send + Sync,
>(
    client: &C,
    templates: &R,
    template_id: TemplateId,
    worker_name: WorkerName,
) -> Result<Template, GolemError> {
    let worker_meta = client
        .get_metadata(worker_name, template_id.clone())
        .await?;

    templates
        .get_metadata(&template_id, worker_meta.template_version)
        .await
}

async fn to_invoke_result_view<C: WorkerClient + Send + Sync, R: TemplateHandler + Send + Sync>(
    res: InvokeResult,
    join_meta: JoinHandle<Result<Template, GolemError>>,
    client: &C,
    templates: &R,
    template_id: &TemplateId,
    worker_name: &WorkerName,
    function: &str,
) -> Result<InvokeResultView, GolemError> {
    let template_meta = match join_meta.await.unwrap() {
        Ok(template_meta) => template_meta,
        Err(_) => {
            // If `get_metadata` might fail if `invoke_and_await` creates a new worker after `get_metadata` call
            resolve_worker_template_version(client, templates, template_id.clone(), worker_name.clone())
                .await?
        }
    };

    Ok(InvokeResultView::try_parse_or_json(
        res,
        &template_meta,
        function,
    ))
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
                human_readable,
            } => {
                let template_id = self.templates.resolve_id(template_id_or_name).await?;

                let join_template_meta = if human_readable {
                    Some(tokio::spawn(resolve_worker_template_version_no_ref(
                        self.worker_context.clone(),
                        self.template_context.clone(),
                        self.allow_insecure,
                        template_id.clone(),
                        worker_name.clone(),
                    )))
                } else {
                    None
                };

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
                        worker_name.clone(),
                        template_id.clone(),
                        function.clone(),
                        InvokeParameters { params: parameters },
                        invocation_key,
                        use_stdio,
                    )
                    .await?;

                match join_template_meta {
                    None => Ok(GolemResult::Json(res.result)),
                    Some(join_template_meta) => {
                        let view = to_invoke_result_view(
                            res,
                            join_template_meta,
                            &self.client,
                            self.templates,
                            &template_id,
                            &worker_name,
                            &function,
                        )
                            .await?;

                        Ok(GolemResult::Ok(Box::new(view)))
                    }
                }
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

                if count.is_some() {
                    let response = self
                        .client
                        .list_metadata(template_id, filter, cursor, count, precise)
                        .await?;

                    Ok(GolemResult::Ok(Box::new(response)))
                } else {
                    let mut workers: Vec<WorkerMetadata> = vec![];
                    let mut new_cursor = cursor;

                    loop {
                        let response = self
                            .client
                            .list_metadata(
                                template_id.clone(),
                                filter.clone(),
                                new_cursor,
                                Some(50),
                                precise,
                            )
                            .await?;

                        workers.extend(response.workers);

                        new_cursor = response.cursor;

                        if new_cursor.is_none() {
                            break;
                        }
                    }

                    Ok(GolemResult::Ok(Box::new(WorkersMetadataResponse {
                        workers,
                        cursor: None,
                    })))
                }
            }
        }
    }
}
