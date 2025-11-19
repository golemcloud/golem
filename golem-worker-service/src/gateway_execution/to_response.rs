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

use super::auth_call_back_binding_handler::{AuthenticationSuccess, AuthorisationError};
use super::file_server_binding_handler::FileServerBindingSuccess;
use super::http_handler_binding_handler::{HttpHandlerBindingError, HttpHandlerBindingSuccess};
use super::swagger_binding_handler::{SwaggerBindingError, SwaggerHtml};
use super::{RibInputTypeMismatch, WorkerRequestExecutorError};
use crate::api::common::ApiEndpointError;
use crate::gateway_execution::file_server_binding_handler::FileServerBindingError;
use crate::gateway_execution::gateway_session_store::GatewaySessionStore;
use crate::gateway_execution::request::RichRequest;
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use async_trait::async_trait;
use golem_service_base::custom_api::HttpCors;
use http::header::*;
use http::StatusCode;
use poem::Body;
use poem::IntoResponse;
use rib::RibResult;
use std::sync::Arc;

#[async_trait]
pub trait ToHttpResponse {
    async fn to_response(
        self,
        request: &RichRequest,
        session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response;
}

#[async_trait]
impl<T: ToHttpResponse + Send, E: ToHttpResponse + Send> ToHttpResponse for Result<T, E> {
    async fn to_response(
        self,
        request: &RichRequest,
        session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        match self {
            Ok(t) => t.to_response(request, session_store).await,
            Err(e) => e.to_response(request, session_store).await,
        }
    }
}

pub type GatewayHttpResult<T> = Result<T, GatewayHttpError>;

pub enum GatewayHttpError {
    BadRequest(String),
    InternalError(String),
    RibInputTypeMismatch(RibInputTypeMismatch),
    EvaluationError(WorkerRequestExecutorError),
    RibInterpretPureError(String),
    HttpHandlerBindingError(HttpHandlerBindingError),
    FileServerBindingError(FileServerBindingError),
    AuthorisationError(AuthorisationError),
}

#[async_trait]
impl ToHttpResponse for GatewayHttpError {
    async fn to_response(
        self,
        request_details: &RichRequest,
        session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        match self {
            GatewayHttpError::BadRequest(e) => poem::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string(e)),
            GatewayHttpError::RibInputTypeMismatch(err) => {
                err.to_response_from_safe_display(|_| StatusCode::BAD_REQUEST)
            }
            GatewayHttpError::RibInterpretPureError(err) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!(
                    "Failed interpreting pure rib expression: {err}"
                ))),
            GatewayHttpError::EvaluationError(err) => {
                err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
            }
            GatewayHttpError::HttpHandlerBindingError(inner) => {
                inner.to_response(request_details, session_store).await
            }
            GatewayHttpError::FileServerBindingError(inner) => {
                inner.to_response(request_details, session_store).await
            }
            GatewayHttpError::AuthorisationError(inner) => {
                inner.to_response(request_details, session_store).await
            }
            GatewayHttpError::InternalError(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(e)),
        }
    }
}

#[async_trait]
impl ToHttpResponse for FileServerBindingSuccess {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        Body::from_bytes_stream(self.data)
            .with_content_type(self.binding_details.content_type.to_string())
            .with_status(self.binding_details.status_code)
            .into_response()
    }
}

#[async_trait]
impl ToHttpResponse for FileServerBindingError {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        match self {
            FileServerBindingError::InternalError(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {e}"))),
            FileServerBindingError::ComponentServiceError(inner) => {
                ApiEndpointError::from(inner).into_response()
            }
            FileServerBindingError::WorkerServiceError(inner) => {
                ApiEndpointError::from(inner).into_response()
            }
            FileServerBindingError::InvalidRibResult(e) => poem::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string(format!(
                    "Error while processing rib result: {e}"
                ))),
        }
    }
}

#[async_trait]
impl ToHttpResponse for HttpHandlerBindingSuccess {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        self.response
    }
}

#[async_trait]
impl ToHttpResponse for HttpHandlerBindingError {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        match self {
            HttpHandlerBindingError::InternalError(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {e}"))),
            HttpHandlerBindingError::WorkerRequestExecutorError(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!(
                    "Error calling worker executor {e}"
                ))),
        }
    }
}

// Preflight (OPTIONS) response that will consist of all configured CORS headers
#[async_trait]
impl ToHttpResponse for HttpCors {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        let mut response = poem::Response::builder().status(StatusCode::OK).finish();

        // TODO: should not unwrap here
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            self.allow_origin.clone().parse().unwrap(),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            self.allow_methods.clone().parse().unwrap(),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            self.allow_headers.clone().parse().unwrap(),
        );

        if let Some(expose_headers) = &self.expose_headers {
            response.headers_mut().insert(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                expose_headers.clone().parse().unwrap(),
            );
        }

        if let Some(allow_credentials) = self.allow_credentials {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                allow_credentials.to_string().parse().unwrap(),
            );
        }

        if let Some(max_age) = self.max_age {
            response
                .headers_mut()
                .insert(ACCESS_CONTROL_MAX_AGE, max_age.to_string().parse().unwrap());
        }

        response
    }
}

#[async_trait]
impl ToHttpResponse for RibResult {
    async fn to_response(
        self,
        request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        match internal::IntermediateRibResultHttpResponse::from(&self) {
            Ok(intermediate_response) => intermediate_response.to_http_response(request_details),
            Err(e) => e.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
}

#[async_trait]
impl ToHttpResponse for AuthenticationSuccess {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        let access_token = self.access_token;
        let id_token = self.id_token;
        let session_id = self.session;

        let mut response = poem::Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", self.target_path)
            .header(
                "Set-Cookie",
                format!("access_token={access_token}; HttpOnly; Secure; Path=/; SameSite=None")
                    .as_str(),
            );

        if let Some(id_token) = id_token {
            response = response.header(
                "Set-Cookie",
                format!("id_token={id_token}; HttpOnly; Secure; Path=/; SameSite=None").as_str(),
            )
        }

        response = response.header(
            "Set-Cookie",
            format!("session_id={session_id}; HttpOnly; Secure; Path=/; SameSite=None").as_str(),
        );

        response.body(())
    }
}

#[async_trait]
impl ToHttpResponse for AuthorisationError {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        self.to_response_from_safe_display(|_| StatusCode::UNAUTHORIZED)
    }
}

#[async_trait]
impl ToHttpResponse for SwaggerHtml {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        poem::Response::builder()
            .content_type("text/html")
            .body(Body::from_string(self.0))
    }
}

#[async_trait]
impl ToHttpResponse for SwaggerBindingError {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &Arc<dyn GatewaySessionStore>,
    ) -> poem::Response {
        self.into()
    }
}

mod internal {
    use crate::gateway_execution::http_content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use crate::gateway_execution::request::RichRequest;
    use http::StatusCode;

    use crate::getter::{get_response_headers_or_default, get_status_code_or_ok, GetterExt};
    use crate::path::Path;

    use crate::gateway_execution::WorkerRequestExecutorError;
    use crate::headers::ResolvedResponseHeaders;
    use golem_wasm::ValueAndType;
    use poem::{Body, IntoResponse, ResponseParts};
    use rib::RibResult;

    #[derive(Debug)]
    pub(crate) struct IntermediateRibResultHttpResponse {
        body: Option<ValueAndType>,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateRibResultHttpResponse {
        pub(crate) fn from(
            evaluation_result: &RibResult,
        ) -> Result<IntermediateRibResultHttpResponse, WorkerRequestExecutorError> {
            match evaluation_result {
                RibResult::Val(rib_result) => {
                    let status =
                        get_status_code_or_ok(rib_result).map_err(WorkerRequestExecutorError)?;

                    let headers = get_response_headers_or_default(rib_result)
                        .map_err(WorkerRequestExecutorError)?;

                    let body = rib_result
                        .get_optional(&Path::from_key("body"))
                        .unwrap_or(rib_result.clone());

                    Ok(IntermediateRibResultHttpResponse {
                        body: Some(body),
                        status,
                        headers,
                    })
                }
                RibResult::Unit => Ok(IntermediateRibResultHttpResponse {
                    body: None,
                    status: StatusCode::default(),
                    headers: ResolvedResponseHeaders::default(),
                }),
            }
        }

        pub(crate) fn to_http_response(&self, request_details: &RichRequest) -> poem::Response {
            let response_content_type = self.headers.get_content_type();
            let response_headers = self.headers.headers.clone();

            let status = &self.status;
            let evaluation_result = &self.body;

            let accepted_content_types = request_details
                .underlying
                .header(http::header::ACCEPT)
                .map(|s| s.to_string());

            let content_type =
                ContentTypeHeaders::from(response_content_type, accepted_content_types);

            let response = match evaluation_result {
                Some(type_annotated_value) => {
                    match type_annotated_value.to_http_resp_with_content_type(content_type) {
                        Ok(body_with_header) => {
                            let mut response = body_with_header.into_response();
                            response.set_status(*status);
                            response.headers_mut().extend(response_headers);
                            response
                        }
                        Err(content_map_error) => poem::Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Body::from_string(content_map_error.to_string())),
                    }
                }
                None => {
                    let parts = ResponseParts {
                        status: *status,
                        version: Default::default(),
                        headers: response_headers,
                        extensions: Default::default(),
                    };

                    poem::Response::from_parts(parts, Body::empty())
                }
            };

            response
        }
    }
}

#[cfg(test)]
mod test {
    use async_trait::async_trait;
    use std::sync::Arc;
    use test_r::test;

    use crate::gateway_execution::gateway_session_store::{
        DataKey, DataValue, GatewaySessionError, GatewaySessionStore, SessionId,
    };
    use crate::gateway_execution::request::RichRequest;
    use crate::gateway_execution::to_response::ToHttpResponse;
    use golem_wasm::analysis::analysed_type::record;
    use golem_wasm::analysis::NameTypePair;
    use golem_wasm::{IntoValueAndType, Value, ValueAndType};
    use http::header::CONTENT_TYPE;
    use http::StatusCode;
    use rib::RibResult;

    fn create_record(values: Vec<(String, ValueAndType)>) -> ValueAndType {
        let mut fields = vec![];
        let mut field_values = vec![];

        for (key, vnt) in values {
            fields.push(NameTypePair {
                name: key,
                typ: vnt.typ,
            });
            field_values.push(vnt.value);
        }

        ValueAndType {
            value: Value::Record(field_values),
            typ: record(fields),
        }
    }

    fn test_request() -> RichRequest {
        RichRequest::new(poem::Request::default())
    }

    #[test]
    async fn test_evaluation_result_to_response_with_http_specifics() {
        let record = create_record(vec![
            ("status".to_string(), 400u16.into_value_and_type()),
            (
                "headers".to_string(),
                create_record(vec![(
                    "Content-Type".to_string(),
                    "application/json".into_value_and_type(),
                )]),
            ),
            ("body".to_string(), "Hello".into_value_and_type()),
        ]);

        let evaluation_result: RibResult = RibResult::Val(record);

        let session_store: Arc<dyn GatewaySessionStore> = Arc::new(TestSessionStore);

        let http_response: poem::Response = evaluation_result
            .to_response(&test_request(), &session_store)
            .await;

        let (response_parts, body) = http_response.into_parts();
        let body = body.into_string().await.unwrap();
        let headers = response_parts.headers;
        let status = response_parts.status;

        let expected_body = "\"Hello\"";
        let expected_headers = poem::web::headers::HeaderMap::from_iter(vec![(
            CONTENT_TYPE,
            "application/json".parse().unwrap(),
        )]);

        let expected_status = StatusCode::BAD_REQUEST;

        assert_eq!(body, expected_body);
        assert_eq!(headers.clone(), expected_headers);
        assert_eq!(status, expected_status);
    }

    #[test]
    async fn test_evaluation_result_to_response_with_no_http_specifics() {
        let evaluation_result: RibResult = RibResult::Val("Healthy".into_value_and_type());

        let session_store: Arc<dyn GatewaySessionStore> = Arc::new(TestSessionStore);

        let http_response: poem::Response = evaluation_result
            .to_response(&test_request(), &session_store)
            .await;

        let (response_parts, body) = http_response.into_parts();
        let body = body.into_string().await.unwrap();
        let headers = response_parts.headers;
        let status = response_parts.status;

        let expected_body = "Healthy";

        // Deault content response is application/json. Refer HttpResponse
        let expected_headers = poem::web::headers::HeaderMap::from_iter(vec![(
            CONTENT_TYPE,
            "application/json".parse().unwrap(),
        )]);
        let expected_status = StatusCode::OK;

        assert_eq!(body, expected_body);
        assert_eq!(headers.clone(), expected_headers);
        assert_eq!(status, expected_status);
    }

    struct TestSessionStore;

    #[async_trait]
    impl GatewaySessionStore for TestSessionStore {
        async fn insert(
            &self,
            _session_id: SessionId,
            _data_key: DataKey,
            _data_value: DataValue,
        ) -> Result<(), GatewaySessionError> {
            Err(GatewaySessionError::InternalError(
                "unimplemented".to_string(),
            ))
        }

        async fn get(
            &self,
            _session_id: &SessionId,
            _data_key: &DataKey,
        ) -> Result<DataValue, GatewaySessionError> {
            Err(GatewaySessionError::InternalError(
                "unimplemented".to_string(),
            ))
        }
    }
}
