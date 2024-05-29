use std::collections::HashMap;
use std::sync::Arc;

use cloud_api_grpc::proto::golem::cloud::limit::cloud_limits_service_server::CloudLimitsService;
use cloud_api_grpc::proto::golem::cloud::limit::{
    batch_update_resource_limits_response, get_resource_limits_response,
    update_worker_limit_response, BatchUpdateResourceLimitsRequest,
    BatchUpdateResourceLimitsResponse, GetResourceLimitsRequest, GetResourceLimitsResponse,
    UpdateWorkerLimitRequest, UpdateWorkerLimitResponse,
};
use cloud_api_grpc::proto::golem::cloud::limit::{limits_error, LimitsError};
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody, ResourceLimits};
use golem_common::model::AccountId;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};

impl From<AuthServiceError> for LimitsError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                limits_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                limits_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        LimitsError { error: Some(error) }
    }
}

impl From<PlanLimitError> for LimitsError {
    fn from(value: PlanLimitError) -> Self {
        let error = match value {
            PlanLimitError::ComponentIdNotFound(_) => limits_error::Error::BadRequest(ErrorsBody {
                errors: vec!["Component not found".to_string()],
            }),
            PlanLimitError::ProjectIdNotFound(_) => limits_error::Error::BadRequest(ErrorsBody {
                errors: vec!["Project not found".to_string()],
            }),
            PlanLimitError::AccountIdNotFound(_) => limits_error::Error::BadRequest(ErrorsBody {
                errors: vec!["Account not found".to_string()],
            }),
            PlanLimitError::Unauthorized(error) => {
                limits_error::Error::Unauthorized(ErrorBody { error })
            }
            PlanLimitError::LimitExceeded(error) => {
                limits_error::Error::LimitExceeded(ErrorBody { error })
            }
            PlanLimitError::Internal(error) => {
                limits_error::Error::InternalError(ErrorBody { error })
            }
        };
        LimitsError { error: Some(error) }
    }
}

fn bad_request_error<T>(error: T) -> LimitsError
where
    T: Into<String>,
{
    LimitsError {
        error: Some(limits_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
        })),
    }
}

pub struct LimitsGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
}

impl LimitsGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, LimitsError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(LimitsError {
                error: Some(limits_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn get(
        &self,
        request: GetResourceLimitsRequest,
        metadata: MetadataMap,
    ) -> Result<ResourceLimits, LimitsError> {
        let auth = self.auth(metadata).await?;

        let account_id: AccountId =
            request
                .account_id
                .map(|a| a.into())
                .ok_or_else(|| LimitsError {
                    error: Some(limits_error::Error::BadRequest(ErrorsBody {
                        errors: vec!["Missing account id".into()],
                    })),
                })?;

        let limits = self
            .plan_limit_service
            .get_resource_limits(&account_id, &auth)
            .await?;

        Ok(limits.into())
    }

    async fn update(
        &self,
        request: BatchUpdateResourceLimitsRequest,
        metadata: MetadataMap,
    ) -> Result<(), LimitsError> {
        let auth = self.auth(metadata).await?;
        let mut updates: HashMap<AccountId, i64> = HashMap::new();
        if let Some(batch_updates) = request.resource_limits {
            for (k, v) in batch_updates.updates {
                updates.insert(AccountId::from(k.as_str()), v);
            }
        }

        self.plan_limit_service
            .record_fuel_consumption(updates, &auth)
            .await?;

        Ok(())
    }

    async fn update_worker_limit(
        &self,
        request: UpdateWorkerLimitRequest,
        metadata: MetadataMap,
    ) -> Result<(), LimitsError> {
        let auth = self.auth(metadata).await?;
        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        self.plan_limit_service
            .update_worker_limit(&account_id, request.value, &auth)
            .await?;

        Ok(())
    }

    async fn update_worker_connection_limit(
        &self,
        request: UpdateWorkerLimitRequest,
        metadata: MetadataMap,
    ) -> Result<(), LimitsError> {
        let auth = self.auth(metadata).await?;
        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        self.plan_limit_service
            .update_worker_connection_limit(&account_id, request.value, &auth)
            .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl CloudLimitsService for LimitsGrpcApi {
    async fn update_worker_limit(
        &self,
        request: Request<UpdateWorkerLimitRequest>,
    ) -> Result<Response<UpdateWorkerLimitResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.update_worker_limit(r, m).await {
            Ok(_) => Ok(Response::new(UpdateWorkerLimitResponse {
                result: Some(update_worker_limit_response::Result::Success(Empty {})),
            })),
            Err(err) => Ok(Response::new(UpdateWorkerLimitResponse {
                result: Some(update_worker_limit_response::Result::Error(err)),
            })),
        }
    }

    async fn update_worker_connection_limit(
        &self,
        request: Request<UpdateWorkerLimitRequest>,
    ) -> Result<Response<UpdateWorkerLimitResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.update_worker_connection_limit(r, m).await {
            Ok(_) => Ok(Response::new(UpdateWorkerLimitResponse {
                result: Some(update_worker_limit_response::Result::Success(Empty {})),
            })),
            Err(err) => Ok(Response::new(UpdateWorkerLimitResponse {
                result: Some(update_worker_limit_response::Result::Error(err)),
            })),
        }
    }

    async fn get_resource_limits(
        &self,
        request: Request<GetResourceLimitsRequest>,
    ) -> Result<Response<GetResourceLimitsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r, m).await {
            Ok(result) => Ok(Response::new(GetResourceLimitsResponse {
                result: Some(get_resource_limits_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetResourceLimitsResponse {
                result: Some(get_resource_limits_response::Result::Error(err)),
            })),
        }
    }

    async fn batch_update_resource_limits(
        &self,
        request: Request<BatchUpdateResourceLimitsRequest>,
    ) -> Result<Response<BatchUpdateResourceLimitsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.update(r, m).await {
            Ok(_) => Ok(Response::new(BatchUpdateResourceLimitsResponse {
                result: Some(batch_update_resource_limits_response::Result::Success(
                    Empty {},
                )),
            })),
            Err(err) => Ok(Response::new(BatchUpdateResourceLimitsResponse {
                result: Some(batch_update_resource_limits_response::Result::Error(err)),
            })),
        }
    }
}
