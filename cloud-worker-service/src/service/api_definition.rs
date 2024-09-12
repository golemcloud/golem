use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::ProjectAction;
use golem_common::model::ProjectId;
use golem_worker_service_base::{
    api_definition::{
        http::CompiledHttpApiDefinition, http::HttpApiDefinitionRequest, ApiDefinitionId,
        ApiVersion,
    },
    service::{
        api_definition::{
            ApiDefinitionError as BaseApiDefinitionError,
            ApiDefinitionService as BaseApiDefinitionService,
        },
        http::http_api_definition_validator::RouteValidationError,
    },
};

use crate::service::auth::{AuthService, AuthServiceError, CloudAuthCtx, CloudNamespace};

pub type ApiDefResult<T> = Result<(T, CloudNamespace), ApiDefinitionError>;

#[async_trait]
pub trait ApiDefinitionService {
    async fn create(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition>;

    async fn update(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition>;

    async fn get(
        &self,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Option<CompiledHttpApiDefinition>>;

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
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition>>;

    async fn get_all_versions(
        &self,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition>>;
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDefinitionError {
    #[error(transparent)]
    Auth(#[from] AuthServiceError),
    #[error(transparent)]
    Base(#[from] BaseApiDefinitionError<RouteValidationError>),
}

#[derive(Clone)]
pub struct ApiDefinitionServiceDefault {
    auth_service: Arc<dyn AuthService + Sync + Send>,
    api_definition_service: BaseService,
}

type BaseService = Arc<
    dyn BaseApiDefinitionService<CloudAuthCtx, CloudNamespace, RouteValidationError> + Sync + Send,
>;

impl ApiDefinitionServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService + Sync + Send>,
        api_definition_service: BaseService,
    ) -> Self {
        Self {
            auth_service,
            api_definition_service,
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
    ) -> ApiDefResult<CompiledHttpApiDefinition> {
        let namespace = self
            .auth_service
            .is_authorized(project_id, ProjectAction::CreateApiDefinition, ctx)
            .await?;

        let api_definition_request = definition.clone();
        let api_definition = self
            .api_definition_service
            .create(&api_definition_request, &namespace.clone(), ctx)
            .await?;

        Ok((api_definition, namespace))
    }

    async fn update(
        &self,
        project_id: &ProjectId,
        definition: &HttpApiDefinitionRequest,
        ctx: &CloudAuthCtx,
    ) -> ApiDefResult<CompiledHttpApiDefinition> {
        let namespace = self
            .auth_service
            .is_authorized(project_id, ProjectAction::UpdateApiDefinition, ctx)
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
    ) -> ApiDefResult<Option<CompiledHttpApiDefinition>> {
        let namespace = self
            .auth_service
            .is_authorized(project_id, ProjectAction::ViewApiDefinition, ctx)
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
            .is_authorized(project_id, ProjectAction::DeleteApiDefinition, ctx)
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
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition>> {
        let namespace = self
            .auth_service
            .is_authorized(project_id, ProjectAction::ViewApiDefinition, ctx)
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
    ) -> ApiDefResult<Vec<CompiledHttpApiDefinition>> {
        let namespace = self
            .auth_service
            .is_authorized(project_id, ProjectAction::ViewApiDefinition, ctx)
            .await?;

        let api_definitions = self
            .api_definition_service
            .get_all_versions(api_id, &namespace.clone(), ctx)
            .await?;

        Ok((api_definitions, namespace))
    }
}
