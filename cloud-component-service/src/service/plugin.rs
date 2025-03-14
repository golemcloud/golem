use crate::model::CloudPluginScope;
use crate::service::component::CloudComponentService;
use crate::service::CloudComponentError;
use cloud_common::auth::CloudAuthCtx;
use cloud_common::clients::auth::BaseAuthService;
use cloud_common::model::{CloudPluginOwner, Role};
use golem_common::model::plugin::{PluginDefinition, PluginOwner, PluginScope};
use golem_component_service_base::api::dto;
use golem_component_service_base::model::plugin::{
    AppPluginCreation, PluginDefinitionCreation, PluginTypeSpecificCreation,
    PluginWasmFileReference,
};
use golem_component_service_base::service::plugin::PluginService;
use golem_service_base::replayable_stream::ReplayableStream;
use std::sync::Arc;

pub struct PluginDefinitionCreationWithoutOwner<Scope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificCreation,
    pub scope: Scope,
}

impl<Scope: PluginScope> PluginDefinitionCreationWithoutOwner<Scope> {
    pub fn with_owner<Owner: PluginOwner>(
        self,
        owner: Owner,
    ) -> PluginDefinitionCreation<Owner, Scope> {
        PluginDefinitionCreation {
            name: self.name,
            version: self.version,
            description: self.description,
            icon: self.icon,
            homepage: self.homepage,
            specs: self.specs,
            scope: self.scope,
            owner,
        }
    }
}

impl<Scope: PluginScope> From<dto::PluginDefinitionCreation<Scope>>
    for PluginDefinitionCreationWithoutOwner<Scope>
{
    fn from(value: dto::PluginDefinitionCreation<Scope>) -> Self {
        Self {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: match value.specs {
                dto::PluginTypeSpecificCreation::ComponentTransformer(inner) => {
                    PluginTypeSpecificCreation::ComponentTransformer(inner)
                }
                dto::PluginTypeSpecificCreation::OplogProcessor(inner) => {
                    PluginTypeSpecificCreation::OplogProcessor(inner)
                }
            },
            scope: value.scope,
        }
    }
}

impl<Scope: PluginScope> From<dto::AppPluginDefinitionCreation<Scope>>
    for PluginDefinitionCreationWithoutOwner<Scope>
{
    fn from(value: dto::AppPluginDefinitionCreation<Scope>) -> Self {
        Self {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon.0,
            homepage: value.homepage,
            specs: PluginTypeSpecificCreation::App(AppPluginCreation {
                data: PluginWasmFileReference::Data(value.wasm.boxed()),
            }),
            scope: value.scope,
        }
    }
}

impl<Scope: PluginScope> From<dto::LibraryPluginDefinitionCreation<Scope>>
    for PluginDefinitionCreationWithoutOwner<Scope>
{
    fn from(value: dto::LibraryPluginDefinitionCreation<Scope>) -> Self {
        Self {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon.0,
            homepage: value.homepage,
            specs: PluginTypeSpecificCreation::App(AppPluginCreation {
                data: PluginWasmFileReference::Data(value.wasm.boxed()),
            }),
            scope: value.scope,
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::component::v1::CreatePluginRequest>
    for PluginDefinitionCreationWithoutOwner<CloudPluginScope>
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::component::v1::CreatePluginRequest,
    ) -> Result<Self, Self::Error> {
        let plugin = value.plugin.ok_or("missing plugin definition")?;

        let converted = Self {
            name: plugin.name,
            version: plugin.version,
            description: plugin.description,
            icon: plugin.icon,
            homepage: plugin.homepage,
            specs: plugin.specs.ok_or("missing specs")?.try_into()?,
            scope: plugin.scope.ok_or("missing scope")?.try_into()?,
        };

        Ok(converted)
    }
}

/// Wraps a `PluginService` implementation (defined in `golem-component-service-base`) so that each
/// operation receives a `CloudAuthCtx` and gets authorized
pub struct CloudPluginService {
    base_plugin_service: Arc<dyn PluginService<CloudPluginOwner, CloudPluginScope> + Sync + Send>,
    cloud_component_service: Arc<CloudComponentService>,
    auth_service: Arc<dyn BaseAuthService + Sync + Send>,
}

impl CloudPluginService {
    pub fn new(
        base_plugin_service: Arc<
            dyn PluginService<CloudPluginOwner, CloudPluginScope> + Sync + Send,
        >,
        cloud_component_service: Arc<CloudComponentService>,
        auth_service: Arc<dyn BaseAuthService + Sync + Send>,
    ) -> Self {
        Self {
            base_plugin_service,
            cloud_component_service,
            auth_service,
        }
    }

    pub async fn list_plugins(
        &self,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self.base_plugin_service.list_plugins(&owner).await?)
    }

    pub async fn list_plugins_for_scope(
        &self,
        auth: &CloudAuthCtx,
        scope: &CloudPluginScope,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self
            .base_plugin_service
            .list_plugins_for_scope(
                &owner,
                scope,
                (self.cloud_component_service.clone(), auth.clone()),
            )
            .await?)
    }

    pub async fn list_plugin_versions(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self
            .base_plugin_service
            .list_plugin_versions(&owner, name)
            .await?)
    }

    pub async fn create_plugin(
        &self,
        auth: &CloudAuthCtx,
        definition: PluginDefinitionCreationWithoutOwner<CloudPluginScope>,
    ) -> Result<(), CloudComponentError> {
        let owner = self.authorize(auth, Role::CreatePlugin).await?;
        let definition = definition.with_owner(owner.clone());
        self.base_plugin_service.create_plugin(definition).await?;
        Ok(())
    }

    pub async fn get(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self.base_plugin_service.get(&owner, name, version).await?)
    }

    pub async fn delete(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
        version: &str,
    ) -> Result<(), CloudComponentError> {
        let owner = self.authorize(auth, Role::DeletePlugin).await?;
        self.base_plugin_service
            .delete(&owner, name, version)
            .await?;
        Ok(())
    }

    async fn authorize(
        &self,
        auth: &CloudAuthCtx,
        role: Role,
    ) -> Result<CloudPluginOwner, CloudComponentError> {
        let account_id = self.auth_service.authorize_role(role, auth).await?;
        Ok(CloudPluginOwner { account_id })
    }
}
