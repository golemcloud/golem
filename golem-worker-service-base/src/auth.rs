use std::error::Error;
use std::fmt::{Display, Formatter};

use async_trait::async_trait;
use derive_more::Display;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Display)]
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
    ) -> Result<Namespace, Box<dyn Error>>;
}

pub struct AuthServiceNoop {}

#[async_trait]
impl AuthService<(), ()> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        _permission: Permission,
        _ctx: &(),
    ) -> Result<(), Box<dyn Error>> {
        Ok(())
    }
}

pub struct AuthNoop {}

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

impl AuthService<AuthNoop, CommonNamespace> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        _permission: Permission,
        _ctx: &(),
    ) -> Result<CommonNamespace, Box<dyn Error>> {
        Ok(CommonNamespace::default())
    }
}
