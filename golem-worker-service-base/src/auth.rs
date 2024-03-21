use std::fmt::{Display, Formatter};

use async_trait::async_trait;
use golem_service_base::service::auth::{AuthError, AuthService, Permission};
use serde::Deserialize;

pub struct AuthServiceNoop {}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmptyAuthCtx {}

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
impl AuthService<EmptyAuthCtx, CommonNamespace> for AuthServiceNoop {
    async fn is_authorized(
        &self,
        _permission: Permission,
        _ctx: &EmptyAuthCtx,
    ) -> Result<CommonNamespace, AuthError> {
        Ok(CommonNamespace::default())
    }
}
