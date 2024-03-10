use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectAction;
use cloud_common::model::TokenSecret;
use derive_more::Display;
use golem_common::model::ProjectId;
use serde::{Deserialize, Serialize};
use tonic::Request;
use tracing::debug;
use uuid::Uuid;

use crate::project::ProjectService;

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

pub struct AuthServiceNoop{}

#[async_trait]
impl AuthService<()> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        permission: Permission,
        ctx: &(),
    ) -> Result<bool, Box<dyn Error>> {
        Ok(true)
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
