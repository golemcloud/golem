use std::sync::Arc;

use golem_common::model::AccountId;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::*;
use crate::service::account_grant::{AccountGrantService, AccountGrantServiceError};
use crate::service::auth::{AuthService, AuthServiceError};
use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::Role;
use golem_service_base::model::{ErrorBody, ErrorsBody};

#[derive(ApiResponse)]
pub enum GrantError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, GrantError>;

impl From<AuthServiceError> for GrantError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                GrantError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                GrantError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<AccountGrantServiceError> for GrantError {
    fn from(value: AccountGrantServiceError) -> Self {
        match value {
            AccountGrantServiceError::Unauthorized(error) => {
                GrantError::Unauthorized(Json(ErrorBody { error }))
            }
            AccountGrantServiceError::Unexpected(error) => {
                GrantError::InternalError(Json(ErrorBody { error }))
            }
            AccountGrantServiceError::ArgValidation(errors) => {
                GrantError::BadRequest(Json(ErrorsBody { errors }))
            }
        }
    }
}

pub struct GrantApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_grant_service: Arc<dyn AccountGrantService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/accounts", tag = ApiTags::Grant)]
impl GrantApi {
    #[oai(path = "/:account_id/grants", method = "get")]
    async fn get_grants(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<Role>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let roles = self.account_grant_service.get(&account_id.0, &auth).await?;
        Ok(Json(roles))
    }

    #[oai(path = "/:account_id/grants/:role", method = "get")]
    async fn get_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Role>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let roles = self.account_grant_service.get(&account_id.0, &auth).await?;
        if roles.contains(&role.0) {
            Ok(Json(role.0))
        } else {
            Err(GrantError::NotFound(Json(ErrorBody {
                error: "Role not found".to_string(),
            })))
        }
    }

    #[oai(path = "/:account_id/grants/:role", method = "put")]
    async fn put_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Role>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        self.account_grant_service
            .add(&account_id.0, &role.0, &auth)
            .await?;
        Ok(Json(role.0))
    }

    #[oai(path = "/:account_id/grants/:role", method = "delete")]
    async fn delete_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteGrantResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        self.account_grant_service
            .remove(&account_id.0, &role.0, &auth)
            .await?;

        Ok(Json(DeleteGrantResponse {}))
    }
}
