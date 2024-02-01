use std::sync::Arc;

use cloud_api_grpc::proto::golem::cloud::account::cloud_account_service_server::CloudAccountService;
use cloud_api_grpc::proto::golem::cloud::account::{
    account_create_response, account_delete_response, account_get_plan_response,
    account_get_response, account_update_response, AccountCreateRequest, AccountCreateResponse,
    AccountDeleteRequest, AccountDeleteResponse, AccountGetPlanRequest, AccountGetPlanResponse,
    AccountGetRequest, AccountGetResponse, AccountUpdateRequest, AccountUpdateResponse,
};
use cloud_api_grpc::proto::golem::cloud::account::{account_error, Account, AccountError};
use cloud_api_grpc::proto::golem::cloud::plan::Plan;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::model::AccountId;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::account;
use crate::service::auth::{AuthService, AuthServiceError};

impl From<AuthServiceError> for AccountError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                account_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                account_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        AccountError { error: Some(error) }
    }
}

impl From<account::AccountError> for AccountError {
    fn from(value: account::AccountError) -> Self {
        let error = match value {
            account::AccountError::Unauthorized(error) => {
                account_error::Error::Unauthorized(ErrorBody { error })
            }
            account::AccountError::Unexpected(error) => {
                account_error::Error::InternalError(ErrorBody { error })
            }
            account::AccountError::UnknownAccountId(_) => {
                account_error::Error::NotFound(ErrorBody {
                    error: "Account not found".to_string(),
                })
            }
            account::AccountError::ArgValidation(errors) => {
                account_error::Error::BadRequest(ErrorsBody { errors })
            }
        };
        AccountError { error: Some(error) }
    }
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
        match self.delete(r, m).await {
            Ok(_) => Ok(Response::new(AccountDeleteResponse {
                result: Some(account_delete_response::Result::Success(Empty {})),
            })),
            Err(err) => Ok(Response::new(AccountDeleteResponse {
                result: Some(account_delete_response::Result::Error(err)),
            })),
        }
    }

    async fn get_account(
        &self,
        request: Request<AccountGetRequest>,
    ) -> Result<Response<AccountGetResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r, m).await {
            Ok(v) => Ok(Response::new(AccountGetResponse {
                result: Some(account_get_response::Result::Account(v)),
            })),
            Err(err) => Ok(Response::new(AccountGetResponse {
                result: Some(account_get_response::Result::Error(err)),
            })),
        }
    }

    async fn get_plan(
        &self,
        request: Request<AccountGetPlanRequest>,
    ) -> Result<Response<AccountGetPlanResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get_account_plan(r, m).await {
            Ok(v) => Ok(Response::new(AccountGetPlanResponse {
                result: Some(account_get_plan_response::Result::Plan(v)),
            })),
            Err(err) => Ok(Response::new(AccountGetPlanResponse {
                result: Some(account_get_plan_response::Result::Error(err)),
            })),
        }
    }

    async fn update_account(
        &self,
        request: Request<AccountUpdateRequest>,
    ) -> Result<Response<AccountUpdateResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.update(r, m).await {
            Ok(v) => Ok(Response::new(AccountUpdateResponse {
                result: Some(account_update_response::Result::Account(v)),
            })),
            Err(err) => Ok(Response::new(AccountUpdateResponse {
                result: Some(account_update_response::Result::Error(err)),
            })),
        }
    }

    async fn create_account(
        &self,
        request: Request<AccountCreateRequest>,
    ) -> Result<Response<AccountCreateResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.create(r, m).await {
            Ok(v) => Ok(Response::new(AccountCreateResponse {
                result: Some(account_create_response::Result::Account(v)),
            })),
            Err(err) => Ok(Response::new(AccountCreateResponse {
                result: Some(account_create_response::Result::Error(err)),
            })),
        }
    }
}
