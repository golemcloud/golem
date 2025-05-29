use async_trait::async_trait;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::{AuthServiceError, BaseAuthService, CloudAuthService};
use cloud_common::model::ProjectAction;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    get_component_metadata_response, GetLatestComponentRequest,
};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{AccountId, ComponentId, ProjectId};
use golem_common::retries::with_retries;
use golem_worker_executor::services::golem_config::ComponentServiceGrpcConfig;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;
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

#[derive(Debug, thiserror::Error)]
pub enum DebuggingServiceAuthError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Bad Request: {}", .0.join(", "))]
    BadRequest(Vec<String>),
    #[error("Internal component service error: {0}")]
    Internal(String),
    #[error("Internal error: {0}")]
    FailedGrpcStatus(Status),
    #[error("Internal error: {0}")]
    FailedTransport(tonic::transport::Error),
}

impl From<Status> for DebuggingServiceAuthError {
    fn from(status: Status) -> Self {
        DebuggingServiceAuthError::FailedGrpcStatus(status)
    }
}

impl From<tonic::transport::Error> for DebuggingServiceAuthError {
    fn from(error: tonic::transport::Error) -> Self {
        DebuggingServiceAuthError::FailedTransport(error)
    }
}

impl From<golem_api_grpc::proto::golem::component::v1::ComponentError>
    for DebuggingServiceAuthError
{
    fn from(error: golem_api_grpc::proto::golem::component::v1::ComponentError) -> Self {
        use golem_api_grpc::proto::golem::component::v1::component_error::Error;
        match error.error {
            Some(Error::BadRequest(errors)) => DebuggingServiceAuthError::BadRequest(errors.errors),
            Some(Error::Unauthorized(error)) => {
                DebuggingServiceAuthError::Unauthorized(error.error)
            }
            Some(Error::LimitExceeded(error)) => DebuggingServiceAuthError::Forbidden(error.error),
            Some(Error::NotFound(error)) => DebuggingServiceAuthError::NotFound(error.error),
            Some(Error::AlreadyExists(error)) => DebuggingServiceAuthError::Internal(error.error),
            Some(Error::InternalError(error)) => DebuggingServiceAuthError::Internal(error.error),
            None => DebuggingServiceAuthError::Internal("Unknown error".to_string()),
        }
    }
}

pub struct AuthServiceDefault {
    common_auth: CloudAuthService,
    component_service_grpc_config: ComponentServiceGrpcConfig,
    component_service_client: GrpcClient<ComponentServiceClient<Channel>>,
    component_project_cache: Cache<ComponentId, (), ProjectId, String>,
}

impl AuthServiceDefault {
    pub fn new(
        common_auth: CloudAuthService,
        component_service_grpc_config: ComponentServiceGrpcConfig,
    ) -> Self {
        let component_service_client = GrpcClient::new(
            "auth_service",
            |channel| {
                ComponentServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            component_service_grpc_config.uri(),
            GrpcClientConfig {
                retries_on_unavailable: component_service_grpc_config.retries.clone(),
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
            component_service_grpc_config,
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
        let retries = self.component_service_grpc_config.retries.clone();
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
                                    .call("get_latest_component", move |client| {
                                        let request = GetLatestComponentRequest {
                                            component_id: Some(id.clone().into()),
                                        };
                                        let request = with_metadata(request, metadata.clone());

                                        Box::pin(client.get_latest_component_metadata(request))
                                    })
                                    .await?
                                    .into_inner();

                                match response.result {
                                    None => Err(DebuggingServiceAuthError::Unauthorized(
                                        "Empty response".to_string(),
                                    )),
                                    Some(get_component_metadata_response::Result::Success(
                                        response,
                                    )) => response
                                        .component
                                        .and_then(|c| c.project_id)
                                        .and_then(|id| id.try_into().ok())
                                        .ok_or_else(|| {
                                            DebuggingServiceAuthError::Unauthorized(
                                                "Empty project id".to_string(),
                                            )
                                        }),
                                    Some(get_component_metadata_response::Result::Error(error)) => {
                                        let err = error.into();
                                        Err(err)
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
impl BaseAuthService for AuthServiceDefault {
    async fn get_account(&self, ctx: &CloudAuthCtx) -> Result<AccountId, AuthServiceError> {
        self.common_auth.get_account(ctx).await
    }

    async fn authorize_project_action(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        self.common_auth
            .authorize_project_action(project_id, permission, ctx)
            .await
    }
}

#[async_trait]
impl AuthService for AuthServiceDefault {
    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        let project_id = self.get_project(component_id, ctx).await?;

        self.authorize_project_action(&project_id, permission, ctx)
            .await
    }
}

fn is_retriable(error: &DebuggingServiceAuthError) -> bool {
    matches!(
        error,
        DebuggingServiceAuthError::FailedTransport(_)
            | DebuggingServiceAuthError::FailedGrpcStatus(_)
    )
}

pub fn with_metadata<T, I, K, V>(request: T, metadata: I) -> tonic::Request<T>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut req = tonic::Request::new(request);
    let req_metadata = req.metadata_mut();

    for (key, value) in metadata {
        let key = tonic::metadata::MetadataKey::from_bytes(key.as_ref().as_bytes());
        let value = value.as_ref().parse();
        if let (Ok(key), Ok(value)) = (key, value) {
            req_metadata.insert(key, value);
        }
    }

    req
}
