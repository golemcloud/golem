use std::fmt::{Display, Formatter};

use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::ResourceLimits;
use golem_common::model::AccountId;
use golem_service_base::service::auth::{AuthError, AuthService, Permission};
use serde::Deserialize;

pub struct AuthServiceNoop {}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmptyAuthCtx {}

impl Display for EmptyAuthCtx {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EmptyAuthCtx")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, bincode::Encode, bincode::Decode, Deserialize)]
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
impl<AuthCtx, Namespace: Default> AuthService<AuthCtx, Namespace> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        _permission: Permission,
        _ctx: &AuthCtx,
    ) -> Result<Namespace, AuthError> {
        Ok(Namespace::default())
    }
}

// TODO: Replace with metadata map
pub trait HasMetadata {
    fn get_metadata(&self) -> WorkerMetadata;
}

#[derive(Clone, Debug)]
pub struct WorkerMetadata {
    pub account_id: Option<AccountId>,
    pub limits: Option<ResourceLimits>,
}

impl HasMetadata for CommonNamespace {
    fn get_metadata(&self) -> WorkerMetadata {
        WorkerMetadata {
            account_id: Some(golem_common::model::AccountId {
                value: "-1".to_string(),
            }),
            limits: None,
        }
    }
}
