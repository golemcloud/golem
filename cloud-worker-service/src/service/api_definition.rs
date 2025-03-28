use std::sync::Arc;

use crate::service::auth::AuthService;
use async_trait::async_trait;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::model::ProjectAction;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::{ComponentId, ProjectId};
use golem_service_base::model::ComponentName;
use golem_worker_service_base::service::component::{ComponentService, ComponentServiceError};
use golem_worker_service_base::service::gateway::{
    BoxConversionContext, ComponentView, ConversionContext,
};
use golem_worker_service_base::{
    gateway_api_definition::{
        http::CompiledHttpApiDefinition, http::HttpApiDefinitionRequest, ApiDefinitionId,
        ApiVersion,
    },
    service::gateway::api_definition::{
        ApiDefinitionError as BaseApiDefinitionError,
        ApiDefinitionService as BaseApiDefinitionService,
    },
};
use serde::{Deserialize, Serialize};

pub type ApiDefResult<T> = Result<(T, CloudNamespace), ApiDefinitionError>;

#[async_trait]
pub trait ApiDefinitionService: Send + Sync {
    async fn create(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition<CloudNamespace>>;

    async fn update(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition<CloudNamespace>>;

    async fn get(
        &self,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Option<CompiledHttpApiDefinition<CloudNamespace>>>;

    async fn delete(
        &self,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<()>;

    async fn get_all(
        &self,
        project_id: &ProjectId,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition<CloudNamespace>>>;

    async fn get_all_versions(
        &self,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition<CloudNamespace>>>;

    fn conversion_context<'a>(&'a self, auth_ctx: &'a CloudAuthCtx) -> BoxConversionContext<'a>;
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDefinitionError {
    #[error(transparent)]
    Auth(#[from] AuthServiceError),
    #[error(transparent)]
    Base(#[from] BaseApiDefinitionError),
}

#[derive(Clone)]
pub struct ApiDefinitionServiceDefault {
    auth_service: Arc<dyn AuthService + Sync + Send>,
    api_definition_service: BaseService,
    component_name_cache: ComponentByNameCache,
    component_id_cache: ComponentByIdCache,
    component_service: Arc<dyn ComponentService<CloudAuthCtx>>,
}

type BaseService = Arc<dyn BaseApiDefinitionService<CloudAuthCtx, CloudNamespace> + Sync + Send>;

impl ApiDefinitionServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService + Sync + Send>,
        api_definition_service: BaseService,
        config: &ApiDefinitionServiceConfig,
        component_service: Arc<dyn ComponentService<CloudAuthCtx>>,
    ) -> Self {
        Self {
            auth_service,
            api_definition_service,
            component_name_cache: Cache::new(
                Some(config.component_by_name_cache_size),
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_name",
            ),
            component_id_cache: Cache::new(
                Some(config.component_by_id_cache_size),
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_id",
            ),
            component_service,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiDefinitionServiceConfig {
    component_by_name_cache_size: usize,
    component_by_id_cache_size: usize,
}

impl Default for ApiDefinitionServiceConfig {
    fn default() -> Self {
        Self {
            component_by_name_cache_size: 1024,
            component_by_id_cache_size: 1024,
        }
    }
}

#[async_trait]
impl ApiDefinitionService for ApiDefinitionServiceDefault {
    async fn create(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition<CloudNamespace>> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::CreateApiDefinition, ctx)
            .await?;

        let api_definition = self
            .api_definition_service
            .create(definition, &namespace.clone(), ctx)
            .await?;

        Ok((api_definition, namespace))
    }

    async fn update(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition<CloudNamespace>> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::UpdateApiDefinition, ctx)
            .await?;

        let api_definition_request = definition.clone();
        let api_definition = self
            .api_definition_service
            .update(&api_definition_request, &namespace.clone(), ctx)
            .await?;

        Ok((api_definition, namespace))
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Option<CompiledHttpApiDefinition<CloudNamespace>>> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::ViewApiDefinition, ctx)
            .await?;

        let api_definition = self
            .api_definition_service
            .get(api_definition_id, version, &namespace.clone(), ctx)
            .await?;

        Ok((api_definition, namespace))
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<()> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::DeleteApiDefinition, ctx)
            .await?;

        self.api_definition_service
            .delete(api_definition_id, version, &namespace.clone(), ctx)
            .await?;

        Ok(((), namespace))
    }

    async fn get_all(
        &self,
        project_id: &ProjectId,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition<CloudNamespace>>> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::ViewApiDefinition, ctx)
            .await?;

        let api_definitions = self
            .api_definition_service
            .get_all(&namespace.clone(), ctx)
            .await?;

        Ok((api_definitions, namespace))
    }

    async fn get_all_versions(
        &self,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition<CloudNamespace>>> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::ViewApiDefinition, ctx)
            .await?;

        let api_definitions = self
            .api_definition_service
            .get_all_versions(api_id, &namespace.clone(), ctx)
            .await?;

        Ok((api_definitions, namespace))
    }

    fn conversion_context<'a>(&'a self, auth_ctx: &'a CloudAuthCtx) -> BoxConversionContext<'a> {
        ConversionContextImpl {
            component_service: &self.component_service,
            auth_ctx,
            component_name_cache: &self.component_name_cache,
            component_id_cache: &self.component_id_cache,
        }
        .boxed()
    }
}

type ComponentByNameCache = Cache<ComponentName, (), Option<ComponentView>, String>;

type ComponentByIdCache = Cache<ComponentId, (), Option<ComponentView>, String>;

struct ConversionContextImpl<'a> {
    component_service: &'a Arc<dyn ComponentService<CloudAuthCtx>>,
    auth_ctx: &'a CloudAuthCtx,
    component_name_cache: &'a ComponentByNameCache,
    component_id_cache: &'a ComponentByIdCache,
}

#[async_trait]
impl ConversionContext for ConversionContextImpl<'_> {
    async fn component_by_name(&self, name: &ComponentName) -> Result<ComponentView, String> {
        let name = name.clone();
        let component = self
            .component_name_cache
            .get_or_insert_simple(&name, async || {
                let result = self
                    .component_service
                    .get_by_name(&name, self.auth_ctx)
                    .await;

                match result {
                    Ok(inner) => Ok(Some(inner.into())),
                    Err(ComponentServiceError::NotFound(_)) => Ok(None),
                    Err(e) => Err(format!("Failed to lookup component by name: {e}")),
                }
            })
            .await?;

        if let Some(component) = component {
            // put component into the other cache to save lookups
            let _ = self
                .component_id_cache
                .get_or_insert_simple(&component.id, async || Ok(Some(component.clone())))
                .await;

            Ok(component)
        } else {
            Err(format!("Did not find component for name {name}"))
        }
    }
    async fn component_by_id(&self, component_id: &ComponentId) -> Result<ComponentView, String> {
        let component = self
            .component_id_cache
            .get_or_insert_simple(component_id, async || {
                let result = self
                    .component_service
                    .get_latest(component_id, self.auth_ctx)
                    .await;

                match result {
                    Ok(inner) => Ok(Some(inner.into())),
                    Err(ComponentServiceError::NotFound(_)) => Ok(None),
                    Err(e) => Err(format!("Failed to lookup component by id: {e}")),
                }
            })
            .await?;

        if let Some(component) = component {
            // put component into the other cache to save lookups
            let _ = self
                .component_name_cache
                .get_or_insert_simple(&component.name, async || Ok(Some(component.clone())))
                .await;

            Ok(component)
        } else {
            Err(format!("Did not find component for id {component_id}"))
        }
    }
}
