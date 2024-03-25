use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Every authorisation is based on a permission to a particular context.
// A context can be a simple unit, to a user, namespace, project, account, or
// a mere request from where we can fetch details.
//
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
    // TODO: Do we want to display these errors?
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    View,
    Create,
    Update,
    Delete,
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Permission::View => write!(f, "View"),
            Permission::Create => write!(f, "Create"),
            Permission::Update => write!(f, "Update"),
            Permission::Delete => write!(f, "Delete"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WithNamespace<T, Namespace> {
    pub value: T,
    pub namespace: Namespace,
}

impl<T, Namespace> WithNamespace<T, Namespace> {
    pub fn new(value: T, namespace: Namespace) -> Self {
        Self { value, namespace }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WithAuth<T, AuthCtx> {
    pub value: T,
    pub context: AuthCtx,
}

impl<T, AuthCtx> WithAuth<T, AuthCtx> {
    pub fn new(value: T, context: AuthCtx) -> Self {
        Self { value, context }
    }
}
