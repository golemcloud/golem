use async_trait::async_trait;
use chrono::Utc;
use http::Uri;
use tonic::transport::Channel;

use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    get_component_metadata_response, GetLatestComponentRequest, GetVersionedComponentRequest,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::config::RetryConfig;
use golem_common::model::ComponentId;
use golem_common::retries::with_retries;
use golem_service_base::model::Component;

use crate::service::component::ComponentServiceError;
use crate::service::with_metadata;
use crate::UriBackConversion;

pub type ComponentResult<T> = Result<T, ComponentServiceError>;

#[async_trait]
pub trait ComponentService<AuthCtx> {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: u64,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component>;

    async fn get_latest(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component>;
}

#[derive(Clone)]
pub struct RemoteComponentService {
    client: GrpcClient<ComponentServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl RemoteComponentService {
    pub fn new(uri: Uri, retry_config: RetryConfig) -> Self {
        Self {
            client: GrpcClient::new(
                ComponentServiceClient::new,
                uri.as_http_02(),
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    ..Default::default() // TODO
                },
            ),
            retry_config,
        }
    }
}

#[async_trait]
impl<AuthCtx> ComponentService<AuthCtx> for RemoteComponentService
where
    AuthCtx: IntoIterator<Item = (String, String)> + Clone + Send + Sync,
{
    async fn get_latest(
        &self,
        component_id: &ComponentId,
        metadata: &AuthCtx,
    ) -> ComponentResult<Component> {
        let value = with_retries(
            "component",
            "get_latest",
            Some(component_id.to_string()),
            &self.retry_config,
            &(self.client.clone(), component_id.clone(), metadata.clone()),
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
                        Some(get_component_metadata_response::Result::Success(response)) => {
                            let component_view: Result<
                                golem_service_base::model::Component,
                                ComponentServiceError,
                            > = match response.component {
                                Some(component) => {
                                    let component: golem_service_base::model::Component =
                                        component.clone().try_into().map_err(|_| {
                                            ComponentServiceError::internal(
                                                "Response conversion error",
                                            )
                                        })?;
                                    Ok(component)
                                }
                                None => {
                                    Err(ComponentServiceError::internal("Empty component response"))
                                }
                            };
                            Ok(component_view?)
                        }
                        Some(get_component_metadata_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            is_retriable,
        )
        .await?;

        Ok(value)
    }

    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: u64,
        metadata: &AuthCtx,
    ) -> ComponentResult<Component> {
        let value = with_retries(
            "component",
            "get_component",
            Some(component_id.to_string()),
            &self.retry_config,
            &(self.client.clone(), component_id.clone(), metadata.clone()),
            |(client, id, metadata)| {
                Box::pin(async move {
                    let response = client
                        .call(move |client| {
                            let request = GetVersionedComponentRequest {
                                component_id: Some(id.clone().into()),
                                version,
                            };

                            let request = with_metadata(request, metadata.clone());

                            Box::pin(client.get_component_metadata(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err(ComponentServiceError::internal("Empty response")),

                        Some(get_component_metadata_response::Result::Success(response)) => {
                            let component_view: Result<
                                golem_service_base::model::Component,
                                ComponentServiceError,
                            > = match response.component {
                                Some(component) => {
                                    let component: golem_service_base::model::Component =
                                        component.clone().try_into().map_err(|_| {
                                            ComponentServiceError::internal(
                                                "Response conversion error",
                                            )
                                        })?;
                                    Ok(component)
                                }
                                None => {
                                    Err(ComponentServiceError::internal("Empty component response"))
                                }
                            };
                            Ok(component_view?)
                        }
                        Some(get_component_metadata_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            is_retriable,
        )
        .await?;

        Ok(value)
    }
}

fn is_retriable(error: &ComponentServiceError) -> bool {
    match error {
        ComponentServiceError::Internal(error) => error.is::<tonic::Status>(),
        _ => false,
    }
}

#[derive(Clone, Debug, Default)]
pub struct ComponentServiceNoop {}

impl ComponentServiceNoop {
    pub fn test_component() -> Component {
        use golem_common::model::component_metadata::ComponentMetadata;
        use golem_service_base::model::{ComponentName, VersionedComponentId};

        let id = VersionedComponentId {
            component_id: ComponentId::new_v4(),
            version: 1,
        };

        Component {
            versioned_component_id: id.clone(),
            component_name: ComponentName("test".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
                memories: vec![],
            },
            created_at: Some(Utc::now()),
        }
    }
}

#[async_trait]
impl<AuthCtx> ComponentService<AuthCtx> for ComponentServiceNoop {
    async fn get_by_version(
        &self,
        _component_id: &ComponentId,
        _version: u64,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        Ok(Self::test_component())
    }

    async fn get_latest(
        &self,
        _component_id: &ComponentId,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        Ok(Self::test_component())
    }
}
