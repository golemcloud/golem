use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::service::account_summary;
use crate::service::account_summary::AccountSummaryServiceError;
use crate::service::auth::{AuthService, AuthServiceError};
use cloud_api_grpc::proto::golem::cloud::accountsummary::v1::cloud_account_summary_service_server::CloudAccountSummaryService;
use cloud_api_grpc::proto::golem::cloud::accountsummary::v1::{
    account_summary_error, get_account_count_response, get_accounts_response, AccountSummary,
    AccountSummaryError, GetAccountCountRequest, GetAccountCountResponse, GetAccountsRequest,
    GetAccountsResponse, GetAccountsSuccessResponse,
};
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::recorded_grpc_api_request;
use golem_common::SafeDisplay;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for AccountSummaryError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(_) => {
                account_summary_error::Error::Unauthorized(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. }
            | AuthServiceError::ProjectAccessForbidden { .. } => {
                account_summary_error::Error::Unauthorized(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => {
                account_summary_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        AccountSummaryError { error: Some(error) }
    }
}

impl From<AccountSummaryServiceError> for AccountSummaryError {
    fn from(value: AccountSummaryServiceError) -> Self {
        match value {
            AccountSummaryServiceError::Internal(_) => {
                wrap_error(account_summary_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AccountSummaryServiceError::AuthError(inner) => inner.into(),
        }
    }
}

fn wrap_error(error: account_summary_error::Error) -> AccountSummaryError {
    AccountSummaryError { error: Some(error) }
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
            .into_iter()
            .map(|a| a.into())
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

        let record = recorded_grpc_api_request!("get_account_count",);

        let response = match self.count(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_account_count_response::Result::Success(result)),
            Err(error) => record.fail(
                get_account_count_response::Result::Error(error.clone()),
                &AccountSummaryTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetAccountCountResponse {
            result: Some(response),
        }))
    }

    async fn get_accounts(
        &self,
        request: Request<GetAccountsRequest>,
    ) -> Result<Response<GetAccountsResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("get_accounts",);

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_accounts_response::Result::Success(result)),
            Err(error) => record.fail(
                get_accounts_response::Result::Error(error.clone()),
                &AccountSummaryTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetAccountsResponse {
            result: Some(response),
        }))
    }
}

pub struct AccountSummaryTraceErrorKind<'a>(pub &'a AccountSummaryError);

impl Debug for AccountSummaryTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for AccountSummaryTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                account_summary_error::Error::Unauthorized(_) => "Unauthorized",
                account_summary_error::Error::InternalError(_) => "InternalError",
            },
        }
    }

    fn is_expected(&self) -> bool {
        match &self.0.error {
            None => false,
            Some(error) => match error {
                account_summary_error::Error::Unauthorized(_) => true,
                account_summary_error::Error::InternalError(_) => true,
            },
        }
    }
}
