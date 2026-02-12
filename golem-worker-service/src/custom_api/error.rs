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

use super::oidc::IdentityProviderError;
use super::oidc::session_store::SessionStoreError;
use super::route_resolver::RouteResolverError;
use crate::service::worker::WorkerServiceError;
use golem_common::{SafeDisplay, error_forwarding};

#[derive(Debug, thiserror::Error)]
pub enum RequestHandlerError {
    #[error("Failed parsing value; Provided: {value}; Expected type: {expected}")]
    ValueParsingFailed {
        value: String,
        expected: &'static str,
    },
    #[error("Expected {expected} values to be provided, but found none")]
    MissingValue { expected: &'static str },
    #[error("Expected {expected} values to be provided, but found too many")]
    TooManyValues { expected: &'static str },
    #[error("Header value of {header_name} is not valid ascii")]
    HeaderIsNotAscii { header_name: String },
    #[error("Request body was not valid json: {error}")]
    BodyIsNotValidJson { error: String },
    #[error("Failed parsing json body: [{formatted}]", formatted=.errors.join(","))]
    JsonBodyParsingFailed { errors: Vec<String> },
    #[error("Agent response did not match expected type: {error}")]
    AgentResponseTypeMismatch { error: String },
    #[error("Mime type {mime_type} is not supported. Allowed mime types: [{formatted_mime_types}]", formatted_mime_types=.allowed_mime_types.join(","))]
    UnsupportedMimeType {
        mime_type: String,
        allowed_mime_types: Vec<String>,
    },
    #[error("Unknown OIDC state")]
    UnknownOidcState,
    #[error("OIDC token exchange failed")]
    OidcTokenExchangeFailed,
    #[error("Invariant violated: {msg}")]
    InvariantViolated { msg: &'static str },
    #[error("Resolving route failed: {0}")]
    ResolvingRouteFailed(#[from] RouteResolverError),
    #[error("Invocation failed: {0}")]
    AgentInvocationFailed(#[from] WorkerServiceError),
    #[error("OIDC loging state is associated with a different security scheme")]
    OidcSchemeMismatch,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl RequestHandlerError {
    pub fn invariant_violated(msg: &'static str) -> Self {
        Self::InvariantViolated { msg }
    }
}

impl SafeDisplay for RequestHandlerError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ValueParsingFailed { .. } => self.to_string(),
            Self::MissingValue { .. } => self.to_string(),
            Self::TooManyValues { .. } => self.to_string(),
            Self::HeaderIsNotAscii { .. } => self.to_string(),
            Self::BodyIsNotValidJson { .. } => self.to_string(),
            Self::JsonBodyParsingFailed { .. } => self.to_string(),
            Self::AgentResponseTypeMismatch { .. } => self.to_string(),
            Self::UnsupportedMimeType { .. } => self.to_string(),
            Self::UnknownOidcState => self.to_string(),
            Self::OidcTokenExchangeFailed => self.to_string(),
            Self::OidcSchemeMismatch => self.to_string(),

            Self::InvariantViolated { .. } => "internal error".to_string(),

            Self::ResolvingRouteFailed(inner) => {
                format!("Resolving route failed: {}", inner.to_safe_string())
            }
            Self::AgentInvocationFailed(inner) => {
                format!("Invocation failed: {}", inner.to_safe_string())
            }

            Self::InternalError(_) => "internal error".to_string(),
        }
    }
}

error_forwarding!(
    RequestHandlerError,
    SessionStoreError,
    IdentityProviderError
);
