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

use crate::services::auth::AuthServiceError;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::SafeDisplay;
use poem_openapi::payload::Json;
use poem_openapi::*;

#[derive(ApiResponse, Debug)]
pub enum DebuggingApiError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl From<AuthServiceError> for DebuggingApiError {
    fn from(value: AuthServiceError) -> Self {
        let error = value.to_safe_string();
        match value {
            AuthServiceError::CouldNotAuthenticate => {
                Self::Unauthorized(Json(ErrorBody { error, cause: None }))
            }
            AuthServiceError::DebuggingNotAllowed => {
                Self::Forbidden(Json(ErrorBody { error, cause: None }))
            }
            AuthServiceError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner),
            })),
        }
    }
}
