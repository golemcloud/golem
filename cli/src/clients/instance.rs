use async_trait::async_trait;
use golem_client::model::{ComponentInstance, InstanceMetadata, InvokeParameters, InvokeResult};
use tracing::info;
use crate::clients::CloudAuthentication;
use crate::{InstanceName};
use crate::model::{GolemError, InvocationKey, RawComponentId};

#[async_trait]
pub trait InstanceClient {
    async fn new_instance(
        &self,
        name: InstanceName,
        component_id: RawComponentId,
        args: Vec<String>,
        env: Vec<(String, String)>,
        auth: &CloudAuthentication,
    ) -> Result<ComponentInstance, GolemError>;
    async fn get_invocation_key(&self, name: &InstanceName, component_id: &RawComponentId, auth: &CloudAuthentication) -> Result<InvocationKey, GolemError>;

    async fn invoke_and_await(
        &self,
        name: InstanceName,
        component_id: RawComponentId,
        function: String,
        parameters: InvokeParameters,
        invocation_key: InvocationKey,
        use_stdio: bool,
        auth: &CloudAuthentication,
    ) -> Result<InvokeResult, GolemError>;

    async fn invoke(
        &self,
        name: InstanceName,
        component_id: RawComponentId,
        function: String,
        parameters: InvokeParameters,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError>;

    async fn interrupt(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
    async fn simulated_crash(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
    async fn delete(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
    async fn get_metadata(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<InstanceMetadata, GolemError>;
}

#[derive(Clone)]
pub struct InstantClientLive<C: golem_client::instance::Instance + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::instance::Instance + Send + Sync> InstanceClient for InstantClientLive<C> {
    async fn new_instance(&self, name: InstanceName, component_id: RawComponentId, args: Vec<String>, env: Vec<(String, String)>, auth: &CloudAuthentication) -> Result<ComponentInstance, GolemError> {
        info!("Creating instance {name} of {}", component_id.0);

        let args = if args.is_empty() {
            None
        } else {
            Some(args.join(",")) // TODO: use json
        };

        let env = if env.is_empty() {
            None
        } else {
            Some(env.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<String>>().join(",")) //  TODO use json
        };

        Ok(self.client.launch_new_instance(&component_id.0.to_string(), &name.0, args.as_deref(), env.as_deref(), &auth.header()).await?)
    }

    async fn get_invocation_key(&self, name: &InstanceName, component_id: &RawComponentId, auth: &CloudAuthentication) -> Result<InvocationKey, GolemError> {
        info!("Getting invocation key for {}/{}", component_id.0, name.0);

        let key = self.client.get_invocation_key(&component_id.0.to_string(), &name.0, &auth.header()).await?;

        Ok(key_api_to_cli(key))
    }

    async fn invoke_and_await(&self, name: InstanceName, component_id: RawComponentId, function: String, parameters: InvokeParameters, invocation_key: InvocationKey, use_stdio: bool, auth: &CloudAuthentication) -> Result<InvokeResult, GolemError> {
        info!("Invoke and await for function {function} in {}/{}", component_id.0, name.0);

        let calling_convention = if use_stdio { "stdio" } else { "component" };

        Ok(self.client.invoke_and_await_function(
            &component_id.0.to_string(),
            &name.0,
            &invocation_key.0,
            &function,
            Some(&calling_convention),
            parameters,
            &auth.header(),
        ).await?)
    }

    async fn invoke(&self, name: InstanceName, component_id: RawComponentId, function: String, parameters: InvokeParameters, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Invoke function {function} in {}/{}", component_id.0, name.0);

        Ok(self.client.invoke_function(
            &component_id.0.to_string(),
            &name.0,
            &function,
            parameters,
            &auth.header(),
        ).await?)
    }

    async fn interrupt(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Interrupting {}/{}", component_id.0, name.0);

        Ok(self.client.interrupt_instance(
            &component_id.0.to_string(),
            &name.0,
            Some(false),
            &auth.header(),
        ).await?)
    }

    async fn simulated_crash(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Simulating crash of {}/{}", component_id.0, name.0);

        Ok(self.client.interrupt_instance(
            &component_id.0.to_string(),
            &name.0,
            Some(true),
            &auth.header(),
        ).await?)
    }

    async fn delete(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Deleting instance {}/{}", component_id.0, name.0);

        Ok(self.client.delete_instance(
            &component_id.0.to_string(),
            &name.0,
            &auth.header(),
        ).await?)
    }

    async fn get_metadata(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<InstanceMetadata, GolemError> {
        info!("Getting instance {}/{} metadata", component_id.0, name.0);

        Ok(self.client.get_instance_metadata(
            &component_id.0.to_string(),
            &name.0,
            &auth.header(),
        ).await?)
    }
}

fn key_api_to_cli(key: golem_client::model::InvocationKey) -> InvocationKey {
    InvocationKey(key.value)
}