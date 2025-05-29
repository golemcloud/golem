// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::service::component::ComponentServiceError;
use crate::service::worker::WorkerServiceError;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{
    ErrorBody, ErrorsBody, GolemError, GolemErrorBody, GolemErrorUnknown,
};
use golem_common::SafeDisplay;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tonic::Status;

// The dependents og golem-worker-service-base
// is expected to exposer worker api endpoints
// that can rely on WorkerApiBaseError
// If there are deviations from this (such as extra terms)
// it should be wrapping WorkerApiBaseError instead of repeating
// error types all over the place
#[derive(ApiResponse, Clone, Debug)]
pub enum WorkerApiBaseError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<GolemErrorBody>),
}

impl TraceErrorKind for WorkerApiBaseError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            WorkerApiBaseError::BadRequest(_) => "BadRequest",
            WorkerApiBaseError::NotFound(_) => "NotFound",
            WorkerApiBaseError::AlreadyExists(_) => "AlreadyExists",
            WorkerApiBaseError::Forbidden(_) => "Forbidden",
            WorkerApiBaseError::Unauthorized(_) => "Unauthorized",
            WorkerApiBaseError::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            WorkerApiBaseError::BadRequest(_) => true,
            WorkerApiBaseError::NotFound(_) => true,
            WorkerApiBaseError::AlreadyExists(_) => true,
            WorkerApiBaseError::Forbidden(_) => true,
            WorkerApiBaseError::Unauthorized(_) => true,
            WorkerApiBaseError::InternalError(_) => false,
        }
    }
}

impl From<tonic::transport::Error> for WorkerApiBaseError {
    fn from(value: tonic::transport::Error) -> Self {
        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<Status> for WorkerApiBaseError {
    fn from(value: Status) -> Self {
        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<String> for WorkerApiBaseError {
    fn from(value: String) -> Self {
        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown { details: value }),
        }))
    }
}

impl From<WorkerServiceError> for WorkerApiBaseError {
    fn from(error: WorkerServiceError) -> Self {
        use WorkerServiceError as ServiceError;

        fn internal(details: String) -> WorkerApiBaseError {
            WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown { details }),
            }))
        }

        match error {
            ServiceError::Internal(_) => internal(error.to_safe_string()),
            ServiceError::TypeChecker(_) => WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                errors: vec![error.to_safe_string()],
            })),
            ServiceError::VersionedComponentIdNotFound(_)
            | ServiceError::ComponentNotFound(_)
            | ServiceError::AccountIdNotFound(_)
            | ServiceError::WorkerNotFound(_) => WorkerApiBaseError::NotFound(Json(ErrorBody {
                error: error.to_safe_string(),
            })),
            ServiceError::Golem(golem_error) => match golem_error {
                GolemError::WorkerNotFound(error) => {
                    WorkerApiBaseError::NotFound(Json(ErrorBody {
                        error: error.to_safe_string(),
                    }))
                }
                _ => WorkerApiBaseError::InternalError(Json(GolemErrorBody { golem_error })),
            },
            ServiceError::Component(error) => error.into(),
            ServiceError::InternalCallError(_) => internal(error.to_safe_string()),
            ServiceError::FileNotFound(_) => WorkerApiBaseError::NotFound(Json(ErrorBody {
                error: error.to_safe_string(),
            })),
            ServiceError::BadFileType(_) => WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                errors: vec![error.to_safe_string()],
            })),
        }
    }
}

impl From<ComponentServiceError> for WorkerApiBaseError {
    fn from(value: ComponentServiceError) -> Self {
        match value {
            ComponentServiceError::BadRequest(errors) => {
                WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors }))
            }
            ComponentServiceError::AlreadyExists(error) => {
                WorkerApiBaseError::AlreadyExists(Json(ErrorBody { error }))
            }
            ComponentServiceError::Internal(error) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: error }),
                }))
            }

            ComponentServiceError::NotFound(error) => {
                WorkerApiBaseError::NotFound(Json(ErrorBody { error }))
            }
            ComponentServiceError::Unauthorized(error) => {
                WorkerApiBaseError::Unauthorized(Json(ErrorBody { error }))
            }
            ComponentServiceError::Forbidden(error) => {
                WorkerApiBaseError::Forbidden(Json(ErrorBody { error }))
            }
            ComponentServiceError::FailedGrpcStatus(_) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: value.to_safe_string(),
                    }),
                }))
            }
            ComponentServiceError::FailedTransport(_) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: value.to_safe_string(),
                    }),
                }))
            }
        }
    }
}
