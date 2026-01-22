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

use std::future::Future;
use std::sync::Arc;

use crate::gateway_execution::request_handler::{RequestHandler, RequestHandlerError};
use futures::FutureExt;
use http::StatusCode;
use poem::{Body, Endpoint, Request, Response};

pub struct CustomHttpRequestApi {
    pub request_handler: Arc<RequestHandler>,
}

impl CustomHttpRequestApi {
    pub fn new(request_handler: Arc<RequestHandler>) -> Self {
        Self { request_handler }
    }

    pub async fn execute(&self, request: Request) -> Response {
        self.request_handler
            .handle_request(request)
            .await
            .unwrap_or_else(transform_request_handler_error)
    }
}

impl Endpoint for CustomHttpRequestApi {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}

fn transform_request_handler_error(error: RequestHandlerError) -> Response {
    let status = match &error {
        RequestHandlerError::ValueParsingFailed { .. }
        | RequestHandlerError::MissingValue { .. }
        | RequestHandlerError::TooManyValues { .. }
        | RequestHandlerError::HeaderIsNotAscii { .. }
        | RequestHandlerError::BodyIsNotValidJson { .. }
        | RequestHandlerError::JsonBodyParsingFailed { .. } => StatusCode::BAD_REQUEST,

        RequestHandlerError::ResolvingRouteFailed(_) => StatusCode::NOT_FOUND,

        RequestHandlerError::AgentResponseTypeMismatch { .. }
        | RequestHandlerError::InvariantViolated { .. }
        | RequestHandlerError::AgentInvocationFailed(_)
        | RequestHandlerError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };

    let body = match &error {
        RequestHandlerError::ValueParsingFailed { value, expected } => {
            format!(
                "Failed parsing value '{}', expected type: {}",
                value, expected
            )
        }
        RequestHandlerError::MissingValue { expected } => {
            format!(
                "Expected {} values to be provided, but found none",
                expected
            )
        }
        RequestHandlerError::TooManyValues { expected } => {
            format!(
                "Expected {} values to be provided, but found too many",
                expected
            )
        }
        RequestHandlerError::HeaderIsNotAscii { header_name } => {
            format!("Header '{}' is not valid ASCII", header_name)
        }
        RequestHandlerError::BodyIsNotValidJson { error } => {
            format!("Request body was not valid JSON: {}", error)
        }
        RequestHandlerError::JsonBodyParsingFailed { errors } => {
            format!("Failed parsing JSON body: [{}]", errors.join(", "))
        }
        RequestHandlerError::AgentResponseTypeMismatch { error } => {
            format!("Agent response did not match expected type: {}", error)
        }
        RequestHandlerError::InvariantViolated { msg } => {
            format!("Invariant violated: {}", msg)
        }
        RequestHandlerError::ResolvingRouteFailed(inner) => {
            format!("Resolving route failed: {}", inner)
        }
        RequestHandlerError::AgentInvocationFailed(inner) => {
            format!("Agent invocation failed: {}", inner)
        }
        RequestHandlerError::InternalError(inner) => {
            format!("Internal server error: {}", inner)
        }
    };

    Response::builder().status(status).body(Body::from(body))
}
