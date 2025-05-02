use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use cloud_api_grpc::proto::golem::cloud::limit::v1::cloud_limits_service_server::CloudLimitsService;
use cloud_api_grpc::proto::golem::cloud::limit::v1::{
    batch_update_resource_limits_response, get_resource_limits_response, limits_error,
    update_component_limit_response, update_worker_limit_response,
    BatchUpdateResourceLimitsRequest, BatchUpdateResourceLimitsResponse, GetResourceLimitsRequest,
    GetResourceLimitsResponse, LimitsError, UpdateComponentLimitRequest,
    UpdateComponentLimitResponse, UpdateWorkerLimitRequest, UpdateWorkerLimitResponse,
};
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody, ResourceLimits};
use golem_common::grpc::{
    proto_account_id_string, proto_component_id_string, proto_worker_id_string,
};
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::AccountId;
use golem_common::recorded_grpc_api_request;
use golem_common::SafeDisplay;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for LimitsError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. }
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. } => {
                limits_error::Error::Unauthorized(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => {
                limits_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        LimitsError { error: Some(error) }
    }
}

impl From<PlanLimitError> for LimitsError {
    fn from(value: PlanLimitError) -> Self {
        match value {
            PlanLimitError::AccountNotFound(_) => {
                wrap_error(limits_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            PlanLimitError::ProjectNotFound(_) => {
                wrap_error(limits_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            PlanLimitError::LimitExceeded(_) => {
                wrap_error(limits_error::Error::LimitExceeded(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            PlanLimitError::Internal(_) | PlanLimitError::InternalRepoError(_) => {
                wrap_error(limits_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            PlanLimitError::AuthError(inner) => inner.into(),
        }
    }
}

fn wrap_error(error: limits_error::Error) -> LimitsError {
    LimitsError { error: Some(error) }
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

    async fn update_component_limit(
        &self,
        request: UpdateComponentLimitRequest,
        metadata: MetadataMap,
    ) -> Result<(), LimitsError> {
        let auth = self.auth(metadata).await?;
        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        self.plan_limit_service
            .update_component_limit(&account_id, request.count, request.size, &auth)
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

        let record = recorded_grpc_api_request!(
            "update_worker_limit",
            component_id = proto_worker_id_string(&r.worker_id),
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self
            .update_worker_limit(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(update_worker_limit_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                update_worker_limit_response::Result::Error(error.clone()),
                &LimitsTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateWorkerLimitResponse {
            result: Some(response),
        }))
    }

    async fn update_worker_connection_limit(
        &self,
        request: Request<UpdateWorkerLimitRequest>,
    ) -> Result<Response<UpdateWorkerLimitResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "update_worker_connection_limit",
            component_id = proto_worker_id_string(&r.worker_id),
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self
            .update_worker_connection_limit(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(update_worker_limit_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                update_worker_limit_response::Result::Error(error.clone()),
                &LimitsTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateWorkerLimitResponse {
            result: Some(response),
        }))
    }

    async fn update_component_limit(
        &self,
        request: Request<UpdateComponentLimitRequest>,
    ) -> Result<Response<UpdateComponentLimitResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "update_component_limit",
            component_id = proto_component_id_string(&r.component_id),
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self
            .update_component_limit(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(update_component_limit_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                update_component_limit_response::Result::Error(error.clone()),
                &LimitsTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateComponentLimitResponse {
            result: Some(response),
        }))
    }

    async fn get_resource_limits(
        &self,
        request: Request<GetResourceLimitsRequest>,
    ) -> Result<Response<GetResourceLimitsResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_resource_limits",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_resource_limits_response::Result::Success(result)),
            Err(error) => record.fail(
                get_resource_limits_response::Result::Error(error.clone()),
                &LimitsTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetResourceLimitsResponse {
            result: Some(response),
        }))
    }

    async fn batch_update_resource_limits(
        &self,
        request: Request<BatchUpdateResourceLimitsRequest>,
    ) -> Result<Response<BatchUpdateResourceLimitsResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("batch_update_resource_limits",);

        let response = match self.update(r, m).instrument(record.span.clone()).await {
            Ok(_) => record.succeed(batch_update_resource_limits_response::Result::Success(
                Empty {},
            )),
            Err(error) => record.fail(
                batch_update_resource_limits_response::Result::Error(error.clone()),
                &LimitsTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(BatchUpdateResourceLimitsResponse {
            result: Some(response),
        }))
    }
}

pub struct LimitsTraceErrorKind<'a>(pub &'a LimitsError);

impl Debug for LimitsTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for LimitsTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                limits_error::Error::BadRequest(_) => "BadRequest",
                limits_error::Error::Unauthorized(_) => "Unauthorized",
                limits_error::Error::LimitExceeded(_) => "LimitExceeded",
                limits_error::Error::InternalError(_) => "InternalError",
            },
        }
    }

    fn is_expected(&self) -> bool {
        match &self.0.error {
            None => false,
            Some(error) => match error {
                limits_error::Error::BadRequest(_) => true,
                limits_error::Error::Unauthorized(_) => true,
                limits_error::Error::LimitExceeded(_) => true,
                limits_error::Error::InternalError(_) => false,
            },
        }
    }
}
