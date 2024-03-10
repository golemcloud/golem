use std::error::Error;

use async_trait::async_trait;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use tonic::Request;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Display)]
pub enum Permission {
    View,
    Create,
    Update,
    Delete,
}

// Every authorisation is based on a permission to a particular context.
// A context can be a simple unit, to a user, namespace, project, account, or
// a composite value of all of these.
#[async_trait]
pub trait AuthService<Ctx> {
    async fn is_authorized(
        &self,
        permission: Permission,
        ctx: &Ctx,
    ) -> Result<bool, Box<dyn Error>>;
}

pub struct AuthServiceNoop {}

#[async_trait]
impl AuthService<()> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        _permission: Permission,
        _ctx: &(),
    ) -> Result<bool, Box<dyn Error>> {
        Ok(true)
    }
}
