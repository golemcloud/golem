use std::fmt::{Display, Formatter};

use async_trait::async_trait;
use derive_more::Display;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Display)]
pub enum Permission {
    View,
    Create,
    Update,
    Delete,
}

// Every authorisation is based on a permission to a particular context.
// A context can be a simple unit, to a user, namespace, project, account, or
// a mere request from where we can fetch details.
#[async_trait]
pub trait AuthService<AuthCtx, Namespace> {
    async fn is_authorized(
        &self,
        permission: Permission,
        ctx: &AuthCtx,
    ) -> Result<Namespace, AuthError>;
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Auth {permission} is forbidden: {reason}")]
    Forbidden {
        permission: Permission,
        reason: String,
    },
}

pub struct AuthServiceNoop {}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmptyAuthCtx {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, bincode::Encode, bincode::Decode, serde::Deserialize,
)]
pub struct CommonNamespace(String);

impl Default for CommonNamespace {
    fn default() -> Self {
        CommonNamespace("common".to_string())
    }
}

impl Display for CommonNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[async_trait]
impl AuthService<EmptyAuthCtx, CommonNamespace> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        _permission: Permission,
        _ctx: &EmptyAuthCtx,
    ) -> Result<CommonNamespace, AuthError> {
        Ok(CommonNamespace::default())
    }
}
