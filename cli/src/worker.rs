use async_trait::async_trait;
use clap::Subcommand;
use clap::builder::ValueParser;
use golem_client::model::InvokeParameters;
use crate::clients::CloudAuthentication;
use crate::clients::worker::WorkerClient;
use crate::template::TemplateHandler;
use crate::model::{TemplateIdOrName, GolemError, GolemResult, WorkerName, InvocationKey, JsonValueParser};
use crate::parse_key_val;

#[derive(Subcommand, Debug)]
#[command()]
pub enum WorkerSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    InvocationKey {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,
    },
    #[command()]
    InvokeAndAwait {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,

        #[arg(short = 'k', long)]
        invocation_key: Option<InvocationKey>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser))]
        parameters: serde_json::value::Value,

        #[arg(short = 's', long, default_value_t = false)]
        use_stdio: bool,
    },
    #[command()]
    Invoke {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser))]
        parameters: serde_json::value::Value,
    },
    #[command()]
    Connect {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,
    },
    #[command()]
    Interrupt {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,
    },
    #[command()]
    SimulatedCrash {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,
    },
    #[command()]
    Get {
        #[command(flatten)]
        template_id_or_name: TemplateIdOrName,

        #[arg(short, long)]
        worker_name: WorkerName,
    },
}

#[async_trait]
pub trait WorkerHandler {
    async fn handle(&self, auth: &CloudAuthentication, subcommand: WorkerSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct WorkerHandlerLive<'r, C: WorkerClient + Send + Sync, R: TemplateHandler + Send + Sync> {
    pub client:C,
    pub templates: &'r R
}

#[async_trait]
impl <'r, C: WorkerClient + Send + Sync, R: TemplateHandler + Send + Sync> WorkerHandler for WorkerHandlerLive<'r, C, R> {
    async fn handle(&self, auth: &CloudAuthentication, subcommand: WorkerSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            WorkerSubcommand::Add { template_id_or_name, worker_name, env, args  } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                let inst = self.client.new_worker(worker_name, template_id, args, env, &auth).await?;

                Ok(GolemResult::Ok(Box::new(inst)))
            }
            WorkerSubcommand::InvocationKey { template_id_or_name, worker_name } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                let key = self.client.get_invocation_key(&worker_name, &template_id, &auth).await?;

                Ok(GolemResult::Ok(Box::new(key)))
            }
            WorkerSubcommand::InvokeAndAwait { template_id_or_name, worker_name,  invocation_key, function, parameters, use_stdio  } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                let invocation_key = match invocation_key {
                    None => self.client.get_invocation_key(&worker_name, &template_id, auth).await?,
                    Some(key) => key,
                };
                
                let res = self.client.invoke_and_await(worker_name, template_id, function, InvokeParameters{params: parameters}, invocation_key, use_stdio, auth).await?;

                Ok(GolemResult::Json(res.result))
            }
            WorkerSubcommand::Invoke { template_id_or_name, worker_name, function, parameters } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                self.client.invoke(worker_name, template_id, function, InvokeParameters{params: parameters}, auth).await?;

                Ok(GolemResult::Str("Invoked".to_string()))
            }
            WorkerSubcommand::Connect { template_id_or_name, worker_name } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                self.client.connect(worker_name, template_id, auth).await?;

                Err(GolemError("connect should never complete".to_string()))
            }
            WorkerSubcommand::Interrupt { template_id_or_name, worker_name } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                self.client.interrupt(worker_name, template_id, auth).await?;

                Ok(GolemResult::Str("Interrupted".to_string()))
            }
            WorkerSubcommand::SimulatedCrash { template_id_or_name, worker_name } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                self.client.simulated_crash(worker_name, template_id, auth).await?;

                Ok(GolemResult::Str("Done".to_string()))
            }
            WorkerSubcommand::Delete { template_id_or_name, worker_name } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                self.client.delete(worker_name, template_id, auth).await?;

                Ok(GolemResult::Str("Deleted".to_string()))
            }
            WorkerSubcommand::Get { template_id_or_name, worker_name } => {
                let template_id = self.templates.resolve_id(template_id_or_name, &auth).await?;

                let mata = self.client.get_metadata(worker_name, template_id, auth).await?;

                Ok(GolemResult::Ok(Box::new(mata)))
            }
        }
    }
}