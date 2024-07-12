use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use cloud_common::model::ProjectAction;
use cloud_common::model::TokenSecret;
use golem_api_grpc::proto::golem::component::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::{
    get_component_metadata_response, GetLatestComponentRequest,
};
use golem_common::model::{AccountId, ComponentId, ProjectId};
use golem_common::retries::with_retries;
use golem_worker_service_base::app_config::ComponentServiceConfig;
use golem_worker_service_base::service::component::ComponentServiceError;
use golem_worker_service_base::service::with_metadata;
use http::Uri;
use serde::Deserialize;
use tonic::metadata::MetadataMap;
use tonic::Request;
use tracing::{debug, error};
use uuid::Uuid;

use crate::service::project::{ProjectError, ProjectService};
use crate::UriBackConversion;

#[async_trait]
pub trait AuthService {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError>;

    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError>;
}

#[derive(Clone)]
pub struct CloudAuthService {
    project_service: Arc<dyn ProjectService + Sync + Send>,
    component_service_config: ComponentServiceConfig,
}

impl CloudAuthService {
    pub fn new(
        project_service: Arc<dyn ProjectService + Sync + Send>,
        component_service_config: ComponentServiceConfig,
    ) -> Self {
        Self {
            project_service,
            component_service_config,
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct CloudAuthCtx {
    pub token_secret: TokenSecret,
}

impl CloudAuthCtx {
    pub fn new(token_secret: TokenSecret) -> Self {
        Self { token_secret }
    }
}

impl IntoIterator for CloudAuthCtx {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![(
            "authorization".to_string(),
            format!("Bearer {}", self.token_secret.value),
        )]
        .into_iter()
    }
}

#[test]
fn test_uuid_aut() {
    let uuid = uuid::Uuid::new_v4();
    let metadata = vec![("authorization".to_string(), format!("Bearer {}", uuid))];

    let result = golem_worker_service_base::service::with_metadata((), metadata);
    assert_eq!(1, result.metadata().len())
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Encode, Decode, Deserialize)]
pub struct CloudNamespace {
    pub project_id: ProjectId,
    // project owner account
    pub account_id: AccountId,
}

impl Display for CloudNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.account_id, self.project_id)
    }
}

impl TryFrom<String> for CloudNamespace {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid namespace: {s}"));
        }

        Ok(Self {
            project_id: ProjectId::try_from(parts[1])?,
            account_id: AccountId::from(parts[0]),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[async_trait]
impl AuthService for CloudAuthService {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        let project_actions = self
            .project_service
            .get_actions(project_id, &ctx.token_secret)
            .await?;
        let project_id = project_actions.project_id.clone();
        let account_id: AccountId = project_actions.owner_account_id;
        let actions = project_actions.actions.actions;
        let has_permission = actions.contains(&permission);

        debug!("is_authorized - project_id: {project_id}, action: {permission:?}, actions: {actions:?}, has_permission: {has_permission}");

        if has_permission {
            Ok(CloudNamespace {
                project_id,
                account_id,
            })
        } else {
            Err(AuthServiceError::Forbidden(format!(
                "No permission {permission:?}"
            )))
        }
    }

    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        let project_id = get_project(component_id, ctx, &self.component_service_config).await?;

        self.is_authorized(&project_id, permission, ctx).await
    }
}

impl From<ProjectError> for AuthServiceError {
    fn from(e: ProjectError) -> Self {
        use cloud_api_grpc::proto::golem::cloud::project::project_error;

        match e {
            ProjectError::Server(e) => match e.error {
                Some(e) => match e {
                    project_error::Error::BadRequest(e) => {
                        AuthServiceError::Internal(anyhow::Error::msg(e.errors.join(", ")))
                    }
                    project_error::Error::Unauthorized(e) => {
                        AuthServiceError::Unauthorized(e.error)
                    }
                    project_error::Error::LimitExceeded(e) => AuthServiceError::Forbidden(e.error),
                    project_error::Error::NotFound(e) => AuthServiceError::Forbidden(e.error),
                    project_error::Error::InternalError(e) => {
                        AuthServiceError::Internal(anyhow::Error::msg(e.error))
                    }
                },
                None => AuthServiceError::Internal(anyhow::Error::msg("Empty error")),
            },
            ProjectError::Connection(e) => AuthServiceError::Internal(e.into()),
            ProjectError::Transport(e) => AuthServiceError::Internal(e.into()),
            ProjectError::Unknown(e) => AuthServiceError::Internal(anyhow::Error::msg(e)),
        }
    }
}

pub fn authorised_request<T>(request: T, access_token: &Uuid) -> Request<T> {
    let mut req = Request::new(request);
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", access_token).parse().unwrap(),
    );
    req
}

pub fn get_authorisation_token(metadata: MetadataMap) -> Option<TokenSecret> {
    let auth = metadata
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    match auth {
        Some(a) if a.to_lowercase().starts_with("bearer ") => {
            let t = &a[7..a.len()];
            TokenSecret::from_str(t.trim()).ok()
        }
        _ => None,
    }
}

#[derive(Default)]
pub struct CloudAuthServiceNoop {}

#[async_trait]
impl AuthService for CloudAuthServiceNoop {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        _permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        Ok(CloudNamespace {
            project_id: project_id.clone(),
            account_id: AccountId::from(ctx.token_secret.value.to_string().as_str()),
        })
    }

    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        _permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        Ok(CloudNamespace {
            project_id: ProjectId(component_id.0),
            account_id: AccountId::from(ctx.token_secret.value.to_string().as_str()),
        })
    }
}

async fn get_project(
    component_id: &ComponentId,
    metadata: &CloudAuthCtx,
    config: &ComponentServiceConfig,
) -> Result<ProjectId, AuthServiceError> {
    let uri: Uri = (*config).uri();

    let result = with_retries(
        "component",
        "get_project",
        Some(format!("{component_id}")),
        &config.retries,
        &(uri.clone(), component_id.clone(), metadata.clone()),
        |(uri, id, metadata)| {
            Box::pin(async move {
                let mut client = ComponentServiceClient::connect(uri.as_http_02()).await?;
                let request = GetLatestComponentRequest {
                    component_id: Some(id.clone().into()),
                };
                let request = with_metadata(request, metadata.clone());

                let response = client
                    .get_latest_component_metadata(request)
                    .await?
                    .into_inner();

                match response.result {
                    None => Err(ComponentServiceError::internal("Empty response")),
                    Some(get_component_metadata_response::Result::Success(response)) => response
                        .component
                        .and_then(|c| c.project_id)
                        .and_then(|id| id.try_into().ok())
                        .ok_or_else(|| ComponentServiceError::internal("Empty project id")),
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
        error!(
            "Getting project of component: {} - error: {}",
            component_id, e
        );
        AuthServiceError::Unauthorized("Get project error".to_string())
    })
}

fn is_retriable(error: &ComponentServiceError) -> bool {
    match error {
        ComponentServiceError::Internal(error) => error.is::<tonic::Status>(),
        _ => false,
    }
}
