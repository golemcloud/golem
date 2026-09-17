// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::api::common::ApiEndpointError;
use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::request_handler::RequestHandler;
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_common::base_model::api;
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::error::ErrorBody;
use golem_common::recorded_http_api_request;
use poem::{Endpoint, IntoResponse, Request, Response};
use std::future::Future;
use std::sync::Arc;
use tracing::Instrument;

pub struct CustomApiPoemEndpoint {
    pub request_handler: Arc<RequestHandler>,
}

impl CustomApiPoemEndpoint {
    pub fn new(request_handler: Arc<RequestHandler>) -> Self {
        Self { request_handler }
    }

    pub async fn execute(&self, request: Request) -> Response {
        let record = recorded_http_api_request!("execute",
            method = %request.method(),
            uri = %request.uri()
        );

        let response = self
            .request_handler
            .handle_request(request)
            .instrument(record.span.clone())
            .await;

        match response {
            Ok(response) => record.succeed(response),
            Err(failure) => {
                log_internal_errors(&failure.error);
                let mut error = CustomApiEndpointError::from(failure.error);
                record.fail((), &mut error);
                error_response(error, failure.cors_headers)
            }
        }
    }
}

fn error_response(error: CustomApiEndpointError, cors_headers: http::HeaderMap) -> Response {
    let mut response = error.into_response();
    response.headers_mut().extend(cors_headers);
    response
}

#[derive(Debug)]
enum CustomApiEndpointError {
    Ordinary(ApiEndpointError),
    Raw {
        status: http::StatusCode,
        body: ErrorBody,
        kind: &'static str,
        expected: bool,
    },
}

impl From<RequestHandlerError> for CustomApiEndpointError {
    fn from(error: RequestHandlerError) -> Self {
        let raw = match &error {
            RequestHandlerError::RawRequest(http::StatusCode::EXPECTATION_FAILED) => Some((
                http::StatusCode::EXPECTATION_FAILED,
                api::error_code::REQUEST_VALUE_PARSING_FAILED,
                "ExpectationFailed",
                true,
            )),
            RequestHandlerError::RawRequest(http::StatusCode::NOT_IMPLEMENTED) => Some((
                http::StatusCode::NOT_IMPLEMENTED,
                api::error_code::REQUEST_VALUE_PARSING_FAILED,
                "NotImplemented",
                true,
            )),
            RequestHandlerError::RawBadGateway => Some((
                http::StatusCode::BAD_GATEWAY,
                api::error_code::INTERNAL_AGENT_EXECUTION_FAILED,
                "BadGateway",
                false,
            )),
            RequestHandlerError::RawDeadline => Some((
                http::StatusCode::GATEWAY_TIMEOUT,
                api::error_code::INTERNAL_AGENT_EXECUTION_FAILED,
                "GatewayTimeout",
                false,
            )),
            _ => None,
        };

        match raw {
            Some((status, code, kind, expected)) => Self::Raw {
                status,
                body: ErrorBody {
                    error: error.to_safe_string(),
                    code: code.into(),
                    cause: None,
                },
                kind,
                expected,
            },
            None => Self::Ordinary(error.into()),
        }
    }
}

impl ApiErrorDetails for CustomApiEndpointError {
    fn trace_error_kind(&self) -> &'static str {
        match self {
            Self::Ordinary(error) => error.trace_error_kind(),
            Self::Raw { kind, .. } => kind,
        }
    }

    fn is_expected(&self) -> bool {
        match self {
            Self::Ordinary(error) => error.is_expected(),
            Self::Raw { expected, .. } => *expected,
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        match self {
            Self::Ordinary(error) => error.take_cause(),
            Self::Raw { body, .. } => body.cause.take(),
        }
    }
}

impl IntoResponse for CustomApiEndpointError {
    fn into_response(self) -> Response {
        match self {
            Self::Ordinary(error) => error.into_response(),
            Self::Raw { status, body, .. } => {
                let mut response = poem::web::Json(body).into_response();
                response.set_status(status);
                response
            }
        }
    }
}

impl Endpoint for CustomApiPoemEndpoint {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}

fn log_internal_errors(error: &RequestHandlerError) {
    match error {
        RequestHandlerError::InvariantViolated { msg } => {
            tracing::warn!(
                error = msg,
                "Internal error due to violated invariant while handling request"
            )
        }
        RequestHandlerError::InternalError(inner) => {
            tracing::warn!(
                error = format!("{:?}", inner),
                "Internal error while handling request"
            )
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use test_r::test;

    async fn assert_raw_error(
        error: RequestHandlerError,
        status: http::StatusCode,
        code: &str,
        kind: &str,
        expected: bool,
    ) {
        let error = CustomApiEndpointError::from(error);
        assert_eq!(error.trace_error_kind(), kind);
        assert_eq!(error.is_expected(), expected);

        let mut cors_headers = http::HeaderMap::new();
        cors_headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            http::HeaderValue::from_static("https://example.com"),
        );
        let response = error_response(error, cors_headers);
        assert_eq!(response.status(), status);
        assert_eq!(
            response
                .headers()
                .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&http::HeaderValue::from_static("https://example.com"))
        );
        let body: Value =
            serde_json::from_slice(&response.into_body().into_vec().await.unwrap()).unwrap();
        assert_eq!(body["code"], code);
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        );
        assert_eq!(body["cause"], Value::Null);
    }

    #[test]
    async fn raw_handler_errors_preserve_response_and_metrics() {
        assert_raw_error(
            RequestHandlerError::RawRequest(http::StatusCode::EXPECTATION_FAILED),
            http::StatusCode::EXPECTATION_FAILED,
            api::error_code::REQUEST_VALUE_PARSING_FAILED,
            "ExpectationFailed",
            true,
        )
        .await;
        assert_raw_error(
            RequestHandlerError::RawRequest(http::StatusCode::NOT_IMPLEMENTED),
            http::StatusCode::NOT_IMPLEMENTED,
            api::error_code::REQUEST_VALUE_PARSING_FAILED,
            "NotImplemented",
            true,
        )
        .await;
        assert_raw_error(
            RequestHandlerError::RawBadGateway,
            http::StatusCode::BAD_GATEWAY,
            api::error_code::INTERNAL_AGENT_EXECUTION_FAILED,
            "BadGateway",
            false,
        )
        .await;
        assert_raw_error(
            RequestHandlerError::RawDeadline,
            http::StatusCode::GATEWAY_TIMEOUT,
            api::error_code::INTERNAL_AGENT_EXECUTION_FAILED,
            "GatewayTimeout",
            false,
        )
        .await;
    }

    #[test]
    fn ordinary_errors_reuse_shared_conversion() {
        let error = CustomApiEndpointError::from(RequestHandlerError::MissingValue {
            expected: "query parameter",
        });
        assert_eq!(error.trace_error_kind(), "BadRequest");
        assert!(error.is_expected());
        assert_eq!(
            error.into_response().status(),
            http::StatusCode::BAD_REQUEST
        );
    }
}
