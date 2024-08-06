use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use crate::model::ProjectView;
use crate::service::project::{ProjectError, ProjectService};
use async_trait::async_trait;
use bincode::{Decode, Encode};
use cloud_common::model::ProjectAction;
use cloud_common::model::TokenSecret;
use golem_common::model::{AccountId, ProjectId};
use serde::Deserialize;
use tonic::metadata::MetadataMap;
use tonic::Request;
use tracing::debug;
use uuid::Uuid;

#[async_trait]
pub trait AuthService {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError>;
}

#[derive(Clone)]
pub struct CloudAuthService {
    project_service: Arc<dyn ProjectService + Sync + Send>,
}

impl CloudAuthService {
    pub fn new(project_service: Arc<dyn ProjectService + Sync + Send>) -> Self {
        Self { project_service }
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

// #[test]
// fn test_uuid_aut() {
//     let uuid = uuid::Uuid::new_v4();
//     let metadata = vec![("authorization".to_string(), format!("Bearer {}", uuid))];
//
//     let result = golem_worker_service_base::service::with_metadata((), metadata);
//     assert_eq!(1, result.metadata().len())
// }

#[derive(Clone, Debug, Hash, Eq, PartialEq, Encode, Decode, Deserialize)]
pub struct CloudNamespace {
    pub project_id: ProjectId,
    // project owner account
    pub account_id: AccountId,
}

impl CloudNamespace {
    pub fn new(project_id: ProjectId, account_id: AccountId) -> Self {
        Self {
            project_id,
            account_id,
        }
    }

    pub fn from(project: ProjectView) -> Self {
        CloudNamespace::new(project.id, project.owner_account_id)
    }
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
            Ok(CloudNamespace::new(project_id, account_id))
        } else {
            Err(AuthServiceError::Forbidden(format!(
                "No permission {permission:?}"
            )))
        }
    }
}

impl From<ProjectError> for AuthServiceError {
    fn from(e: ProjectError) -> Self {
        use cloud_api_grpc::proto::golem::cloud::project::v1::project_error;

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

pub struct CloudAuthServiceNoop {}

#[async_trait]
impl AuthService for CloudAuthServiceNoop {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        _permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        Ok(CloudNamespace::new(
            project_id.clone(),
            AccountId::from(ctx.token_secret.value.to_string().as_str()),
        ))
    }
}
