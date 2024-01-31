use std::sync::Arc;
use tonic::{Request, Response, Status};
use golem_api_grpc::proto::golem::common::{ErrorBody};
use golem_api_grpc_cloud::proto::golem::cloud::accountsummary::{AccountSummary, AccountSummaryError};
use golem_api_grpc_cloud::proto::golem::cloud::accountsummary::cloud_account_summary_service_server::CloudAccountSummaryService;
use golem_api_grpc_cloud::proto::golem::cloud::accountsummary::{GetAccountCountRequest, GetAccountCountResponse, GetAccountsRequest, GetAccountsResponse, GetAccountsSuccessResponse};
use golem_api_grpc_cloud::proto::golem::cloud::accountsummary::get_account_count_response;
use golem_api_grpc_cloud::proto::golem::cloud::accountsummary::get_accounts_response;
use golem_api_grpc_cloud::proto::golem::cloud::accountsummary::account_summary_error;
use tonic::metadata::MetadataMap;
use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::account_summary;
use crate::service::account_summary::AccountSummaryServiceError;

impl From<AuthServiceError> for AccountSummaryError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                account_summary_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                account_summary_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        AccountSummaryError { error: Some(error) }
    }
}

impl From<AccountSummaryServiceError> for AccountSummaryError {
    fn from(value: AccountSummaryServiceError) -> Self {
        let error = match value {
            AccountSummaryServiceError::Unauthorized(error) => {
                account_summary_error::Error::Unauthorized(ErrorBody { error })
            }
            AccountSummaryServiceError::Unexpected(error) => {
                account_summary_error::Error::InternalError(ErrorBody { error })
            }
        };
        AccountSummaryError { error: Some(error) }
    }
}

pub struct AccountSummaryGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_summary_service: Arc<dyn account_summary::AccountSummaryService + Sync + Send>,
}

impl AccountSummaryGrpcApi {
    async fn auth(
        &self,
        metadata: MetadataMap,
    ) -> Result<AccountAuthorisation, AccountSummaryError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(AccountSummaryError {
                error: Some(account_summary_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn count(
        &self,
        _request: GetAccountCountRequest,
        metadata: MetadataMap,
    ) -> Result<i64, AccountSummaryError> {
        let auth = self.auth(metadata).await?;
        let value = self.account_summary_service.count(&auth).await?;
        Ok(value as i64)
    }

    async fn get(
        &self,
        request: GetAccountsRequest,
        metadata: MetadataMap,
    ) -> Result<GetAccountsSuccessResponse, AccountSummaryError> {
        let auth = self.auth(metadata).await?;
        let values = self
            .account_summary_service
            .get(request.skip, request.limit, &auth)
            .await?;

        let accounts = values
            .iter()
            .map(|a| a.clone().into())
            .collect::<Vec<AccountSummary>>();

        Ok(GetAccountsSuccessResponse { accounts })
    }
}

#[async_trait::async_trait]
impl CloudAccountSummaryService for AccountSummaryGrpcApi {
    async fn get_account_count(
        &self,
        request: Request<GetAccountCountRequest>,
    ) -> Result<Response<GetAccountCountResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.count(r, m).await {
            Ok(result) => Ok(Response::new(GetAccountCountResponse {
                result: Some(get_account_count_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetAccountCountResponse {
                result: Some(get_account_count_response::Result::Error(err)),
            })),
        }
    }

    async fn get_accounts(
        &self,
        request: Request<GetAccountsRequest>,
    ) -> Result<Response<GetAccountsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r, m).await {
            Ok(result) => Ok(Response::new(GetAccountsResponse {
                result: Some(get_accounts_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetAccountsResponse {
                result: Some(get_accounts_response::Result::Error(err)),
            })),
        }
    }
}
