use crate::api::{ApiError, ApiResult, ApiTags};
use crate::model::*;
use crate::service::account_grant::AccountGrantService;
use crate::service::auth::AuthService;
use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::Role;
use golem_common::model::AccountId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::ErrorBody;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct GrantApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_grant_service: Arc<dyn AccountGrantService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/accounts", tag = ApiTags::Grant)]
impl GrantApi {
    #[oai(
        path = "/:account_id/grants",
        method = "get",
        operation_id = "get_account_grants"
    )]
    async fn get_grants(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<Role>>> {
        let record =
            recorded_http_api_request!("get_account_grants", account_id = account_id.0.to_string());
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let roles = self
                .account_grant_service
                .get(&account_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(roles))
        };

        record.result(response)
    }

    #[oai(
        path = "/:account_id/grants/:role",
        method = "get",
        operation_id = "get_account_grant"
    )]
    async fn get_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        let record =
            recorded_http_api_request!("get_account_grant", account_id = account_id.0.to_string());
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let roles = self
                .account_grant_service
                .get(&account_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            if roles.contains(&role.0) {
                Ok(Json(role.0))
            } else {
                Err(ApiError::NotFound(Json(ErrorBody {
                    error: "Role not found".to_string(),
                })))
            }
        };

        record.result(response)
    }

    #[oai(
        path = "/:account_id/grants/:role",
        method = "put",
        operation_id = "create_account_grant"
    )]
    async fn put_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        let record = recorded_http_api_request!(
            "create_account_grant",
            account_id = account_id.0.to_string()
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            self.account_grant_service
                .add(&account_id.0, &role.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(role.0))
        };

        record.result(response)
    }

    #[oai(
        path = "/:account_id/grants/:role",
        method = "delete",
        operation_id = "delete_account_grant"
    )]
    async fn delete_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeleteGrantResponse>> {
        let record = recorded_http_api_request!(
            "delete_account_grant",
            account_id = account_id.0.to_string()
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            self.account_grant_service
                .remove(&account_id.0, &role.0, &auth)
                .instrument(record.span.clone())
                .await?;

            Ok(Json(DeleteGrantResponse {}))
        };

        record.result(response)
    }
}
