use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::account;
use crate::service::auth::{AuthService, AuthServiceError};
use cloud_api_grpc::proto::golem::cloud::account::v1::cloud_account_service_server::CloudAccountService;
use cloud_api_grpc::proto::golem::cloud::account::v1::{
    account_create_response, account_delete_response, account_error, account_get_plan_response,
    account_get_response, account_update_response, AccountCreateRequest, AccountCreateResponse,
    AccountDeleteRequest, AccountDeleteResponse, AccountError, AccountGetPlanRequest,
    AccountGetPlanResponse, AccountGetRequest, AccountGetResponse, AccountUpdateRequest,
    AccountUpdateResponse,
};
use cloud_api_grpc::proto::golem::cloud::account::Account;
use cloud_api_grpc::proto::golem::cloud::plan::Plan;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::grpc::proto_account_id_string;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::AccountId;
use golem_common::recorded_grpc_api_request;
use golem_common::SafeDisplay;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for AccountError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. }
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. } => {
                account_error::Error::Unauthorized(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => {
                account_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        AccountError { error: Some(error) }
    }
}

impl From<account::AccountError> for AccountError {
    fn from(value: account::AccountError) -> Self {
        match value {
            account::AccountError::Internal(_) => {
                wrap_error(account_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            account::AccountError::AccountNotFound(_) => {
                wrap_error(account_error::Error::NotFound(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            account::AccountError::ArgValidation(errors) => {
                wrap_error(account_error::Error::BadRequest(ErrorsBody { errors }))
            }
            account::AccountError::InternalRepoError(_) => {
                wrap_error(account_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            account::AccountError::InternalPlanError(_) => {
                wrap_error(account_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            account::AccountError::AuthError(inner) => inner.into(),
        }
    }
}

fn wrap_error(error: account_error::Error) -> AccountError {
    AccountError { error: Some(error) }
}

fn bad_request_error(error: &str) -> AccountError {
    AccountError {
        error: Some(account_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

pub struct AccountGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_service: Arc<dyn account::AccountService + Sync + Send>,
}

impl AccountGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, AccountError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(AccountError {
                error: Some(account_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn delete(
        &self,
        request: AccountDeleteRequest,
        metadata: MetadataMap,
    ) -> Result<(), AccountError> {
        let auth = self.auth(metadata).await?;
        let id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;
        self.account_service.delete(&id, &auth).await?;

        Ok(())
    }

    async fn get(
        &self,
        request: AccountGetRequest,
        metadata: MetadataMap,
    ) -> Result<Account, AccountError> {
        let auth = self.auth(metadata).await?;
        let id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;
        let result = self.account_service.get(&id, &auth).await?;
        Ok(result.into())
    }

    async fn create(
        &self,
        request: AccountCreateRequest,
        metadata: MetadataMap,
    ) -> Result<Account, AccountError> {
        let auth = self.auth(metadata).await?;
        let data: model::AccountData = request
            .account_data
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account data"))?;
        let result = self
            .account_service
            .create(&AccountId::generate(), &data, &auth)
            .await?;
        Ok(result.into())
    }

    async fn update(
        &self,
        request: AccountUpdateRequest,
        metadata: MetadataMap,
    ) -> Result<Account, AccountError> {
        let auth = self.auth(metadata).await?;
        let id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;
        let data: model::AccountData = request
            .account_data
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account data"))?;
        let result = self.account_service.update(&id, &data, &auth).await?;
        Ok(result.into())
    }

    async fn get_account_plan(
        &self,
        request: AccountGetPlanRequest,
        metadata: MetadataMap,
    ) -> Result<Plan, AccountError> {
        let auth = self.auth(metadata).await?;
        let id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;
        let result = self.account_service.get_plan(&id, &auth).await?;
        Ok(result.into())
    }
}

#[async_trait::async_trait]
impl CloudAccountService for AccountGrpcApi {
    async fn delete_account(
        &self,
        request: Request<AccountDeleteRequest>,
    ) -> Result<Response<AccountDeleteResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "delete_account",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.delete(r, m).instrument(record.span.clone()).await {
            Ok(_) => record.succeed(account_delete_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                account_delete_response::Result::Error(error.clone()),
                &AccountTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(AccountDeleteResponse {
            result: Some(response),
        }))
    }

    async fn get_account(
        &self,
        request: Request<AccountGetRequest>,
    ) -> Result<Response<AccountGetResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_account",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(account_get_response::Result::Account(result)),
            Err(error) => record.fail(
                account_get_response::Result::Error(error.clone()),
                &AccountTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(AccountGetResponse {
            result: Some(response),
        }))
    }

    async fn get_plan(
        &self,
        request: Request<AccountGetPlanRequest>,
    ) -> Result<Response<AccountGetPlanResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_account_plan",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self
            .get_account_plan(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(account_get_plan_response::Result::Plan(result)),
            Err(error) => record.fail(
                account_get_plan_response::Result::Error(error.clone()),
                &AccountTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(AccountGetPlanResponse {
            result: Some(response),
        }))
    }

    async fn update_account(
        &self,
        request: Request<AccountUpdateRequest>,
    ) -> Result<Response<AccountUpdateResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "update_account",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.update(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(account_update_response::Result::Account(result)),
            Err(error) => record.fail(
                account_update_response::Result::Error(error.clone()),
                &AccountTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(AccountUpdateResponse {
            result: Some(response),
        }))
    }

    async fn create_account(
        &self,
        request: Request<AccountCreateRequest>,
    ) -> Result<Response<AccountCreateResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "create_account",
            account_name = r.account_data.as_ref().map(|data| data.name.clone())
        );

        let response = match self.create(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(account_create_response::Result::Account(result)),
            Err(error) => record.fail(
                account_create_response::Result::Error(error.clone()),
                &AccountTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(AccountCreateResponse {
            result: Some(response),
        }))
    }
}

pub struct AccountTraceErrorKind<'a>(pub &'a AccountError);

impl Debug for AccountTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for AccountTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                account_error::Error::BadRequest(_) => "BadRequest",
                account_error::Error::Unauthorized(_) => "Unauthorized",
                account_error::Error::NotFound(_) => "NotFound",
                account_error::Error::InternalError(_) => "InternalError",
            },
        }
    }

    fn is_expected(&self) -> bool {
        match &self.0.error {
            None => false,
            Some(error) => match error {
                account_error::Error::BadRequest(_) => true,
                account_error::Error::Unauthorized(_) => true,
                account_error::Error::NotFound(_) => true,
                account_error::Error::InternalError(_) => false,
            },
        }
    }
}
