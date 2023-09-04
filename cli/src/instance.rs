use async_trait::async_trait;
use clap::Subcommand;
use golem_client::model::InvokeParameters;
use crate::clients::CloudAuthentication;
use crate::clients::instance::InstanceClient;
use crate::component::ComponentHandler;
use crate::model::{ComponentIdOrName, GolemError, GolemResult, InstanceName, InvocationKey};
use crate::parse_key_val;

#[derive(Subcommand, Debug)]
#[command()]
pub enum InstanceSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    InvocationKey {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    InvokeAndAwait {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short = 'k', long)]
        invocation_key: Option<InvocationKey>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long, value_name = "json")]
        parameters: serde_json::value::Value,

        #[arg(short = 's', long, default_value_t = false)]
        use_stdio: bool,
    },
    #[command()]
    Invoke {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long, value_name = "json")]
        parameters: serde_json::value::Value,
    },
    #[command()]
    Connect {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Interrupt {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    SimulatedCrash {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Get {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
}

#[async_trait]
pub trait InstanceHandler {
    async fn handle(&self, auth: &CloudAuthentication, subcommand: InstanceSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct InstanceHandlerLive<'r, C: InstanceClient + Send + Sync, R: ComponentHandler + Send + Sync> {
    pub client:C,
    pub component: &'r R
}

#[async_trait]
impl <'r, C: InstanceClient + Send + Sync, R: ComponentHandler + Send + Sync> InstanceHandler for InstanceHandlerLive<'r, C, R> {
    async fn handle(&self, auth: &CloudAuthentication, subcommand: InstanceSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            InstanceSubcommand::Add { component_id_or_name, instance_name, env, args  } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                let inst = self.client.new_instance(instance_name, component_id, args, env, &auth).await?;

                Ok(GolemResult::Ok(Box::new(inst)))
            }
            InstanceSubcommand::InvocationKey { component_id_or_name, instance_name } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                let key = self.client.get_invocation_key(&instance_name, &component_id, &auth).await?;

                Ok(GolemResult::Ok(Box::new(key)))
            }
            InstanceSubcommand::InvokeAndAwait { component_id_or_name, instance_name,  invocation_key, function, parameters, use_stdio  } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                let invocation_key = match invocation_key {
                    None => self.client.get_invocation_key(&instance_name, &component_id, auth).await?,
                    Some(key) => key,
                };

                let res = self.client.invoke_and_await(instance_name, component_id, function, InvokeParameters{params: parameters}, invocation_key, use_stdio, auth).await?;

                Ok(GolemResult::Json(res.result))
            }
            InstanceSubcommand::Invoke { component_id_or_name, instance_name, function, parameters } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                self.client.invoke(instance_name, component_id, function, InvokeParameters{params: parameters}, auth).await?;

                Ok(GolemResult::Str("Invoked".to_string()))
            }
            InstanceSubcommand::Connect { .. } => {todo!()}
            InstanceSubcommand::Interrupt { component_id_or_name, instance_name } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                self.client.interrupt(instance_name, component_id, auth).await?;

                Ok(GolemResult::Str("Interrupted".to_string()))
            }
            InstanceSubcommand::SimulatedCrash { component_id_or_name, instance_name } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                self.client.simulated_crash(instance_name, component_id, auth).await?;

                Ok(GolemResult::Str("Done".to_string()))
            }
            InstanceSubcommand::Delete { component_id_or_name, instance_name } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                self.client.delete(instance_name, component_id, auth).await?;

                Ok(GolemResult::Str("Deleted".to_string()))
            }
            InstanceSubcommand::Get { component_id_or_name, instance_name } => {
                let component_id = self.component.resolve_id(component_id_or_name, &auth).await?;

                let mata = self.client.get_metadata(instance_name, component_id, auth).await?;

                Ok(GolemResult::Ok(Box::new(mata)))
            }
        }
    }
}