use async_trait::async_trait;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::{AuthServiceError, BaseAuthService};
use cloud_common::clients::project::ProjectService;
use cloud_common::model::ProjectAction;
use cloud_common::UriBackConversion;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    get_component_metadata_response, GetLatestComponentRequest,
};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{ComponentId, ProjectId};
use golem_common::retries::with_retries;
use golem_worker_service_base::app_config::ComponentServiceConfig;
use golem_worker_service_base::service::component::ComponentServiceError;
use golem_worker_service_base::service::with_metadata;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::Channel;
use tracing::error;

#[async_trait]
pub trait AuthService: BaseAuthService {
    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError>;
}

pub struct CloudAuthService {
    common_auth: cloud_common::clients::auth::CloudAuthService,
    component_service_config: ComponentServiceConfig,
    component_service_client: GrpcClient<ComponentServiceClient<Channel>>,
    component_project_cache: Cache<ComponentId, (), ProjectId, String>,
}

impl CloudAuthService {
    pub fn new(
        project_service: Arc<dyn ProjectService + Sync + Send>,
        component_service_config: ComponentServiceConfig,
    ) -> Self {
        let common_auth = cloud_common::clients::auth::CloudAuthService::new(project_service);

        let component_service_client = GrpcClient::new(
            ComponentServiceClient::new,
            component_service_config.uri().as_http_02(),
            GrpcClientConfig {
                retries_on_unavailable: component_service_config.retries.clone(),
                ..Default::default() // TODO
            },
        );

        // TODO configuration
        let component_project_cache = Cache::new(
            Some(10000),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::OlderThan {
                ttl: Duration::from_secs(60 * 60),
                period: Duration::from_secs(60),
            },
            "component_project",
        );

        Self {
            common_auth,
            component_service_config,
            component_service_client,
            component_project_cache,
        }
    }

    async fn get_project(
        &self,
        component_id: &ComponentId,
        metadata: &CloudAuthCtx,
    ) -> Result<ProjectId, AuthServiceError> {
        let id = component_id.clone();
        let metadata = metadata.clone();
        let retries = self.component_service_config.retries.clone();
        let client = self.component_service_client.clone();

        self.component_project_cache
            .get_or_insert_simple(component_id, || {
                Box::pin(async move {
                    let result = with_retries(
                        "component",
                        "get_project",
                        Some(format!("{id}")),
                        &retries.clone(),
                        &(client.clone(), id.clone(), metadata.clone()),
                        |(client, id, metadata)| {
                            Box::pin(async move {
                                let response = client
                                    .call(move |client| {
                                        let request = GetLatestComponentRequest {
                                            component_id: Some(id.clone().into()),
                                        };
                                        let request = with_metadata(request, metadata.clone());

                                        Box::pin(client.get_latest_component_metadata(request))
                                    })
                                    .await?
                                    .into_inner();

                                match response.result {
                                    None => Err(ComponentServiceError::internal("Empty response")),
                                    Some(get_component_metadata_response::Result::Success(
                                        response,
                                    )) => response
                                        .component
                                        .and_then(|c| c.project_id)
                                        .and_then(|id| id.try_into().ok())
                                        .ok_or_else(|| {
                                            ComponentServiceError::internal("Empty project id")
                                        }),
                                    Some(get_component_metadata_response::Result::Error(error)) => {
                                        Err(error.into())
                                    }
                                }
                            })
                        },
                        is_retriable,
                    )
                    .await;

                    result.map_err(|e| {
                        error!("Getting project of component: {} - error: {}", id, e);
                        "Get project error".to_string()
                    })
                })
            })
            .await
            .map_err(AuthServiceError::Unauthorized)
    }
}

#[async_trait]
impl cloud_common::clients::auth::BaseAuthService for CloudAuthService {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        self.common_auth
            .is_authorized(project_id, permission, ctx)
            .await
    }
}

#[async_trait]
impl AuthService for CloudAuthService {
    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        let project_id = self.get_project(component_id, ctx).await?;

        self.is_authorized(&project_id, permission, ctx).await
    }
}

fn is_retriable(error: &ComponentServiceError) -> bool {
    match error {
        ComponentServiceError::Internal(error) => error.is::<tonic::Status>(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use golem_worker_service_base::service::with_metadata;
    use uuid::Uuid;

    #[test]
    fn test_uuid_aut() {
        let uuid = Uuid::new_v4();
        let metadata = vec![("authorization".to_string(), format!("Bearer {}", uuid))];

        let result = with_metadata((), metadata);
        assert_eq!(1, result.metadata().len())
    }
}
