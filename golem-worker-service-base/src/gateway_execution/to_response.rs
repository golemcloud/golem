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

use super::auth_call_back_binding_handler::{AuthorisationError, AuthorisationSuccess};
use super::file_server_binding_handler::FileServerBindingSuccess;
use super::http_handler_binding_handler::{HttpHandlerBindingError, HttpHandlerBindingSuccess};
use super::RibInputTypeMismatch;
use crate::api::WorkerApiBaseError;
use crate::gateway_execution::file_server_binding_handler::FileServerBindingError;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::request::RichRequest;
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_middleware::HttpCors as CorsPreflight;
use crate::gateway_rib_interpreter::RibRuntimeError;
use async_trait::async_trait;
use http::header::*;
use http::StatusCode;
use poem::Body;
use poem::IntoResponse;
use rib::RibResult;

#[async_trait]
pub trait ToHttpResponse {
    async fn to_response(
        self,
        request: &RichRequest,
        session_store: &GatewaySessionStore,
    ) -> poem::Response;
}

#[async_trait]
impl<T: ToHttpResponse + Send, E: ToHttpResponse + Send> ToHttpResponse for Result<T, E> {
    async fn to_response(
        self,
        request: &RichRequest,
        session_store: &GatewaySessionStore,
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
    EvaluationError(RibRuntimeError),
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
        session_store: &GatewaySessionStore,
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
        _session_store: &GatewaySessionStore,
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
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        match self {
            FileServerBindingError::InternalError(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {e}"))),
            FileServerBindingError::ComponentServiceError(inner) => {
                WorkerApiBaseError::from(inner).into_response()
            }
            FileServerBindingError::WorkerServiceError(inner) => {
                WorkerApiBaseError::from(inner).into_response()
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
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        self.response
    }
}

#[async_trait]
impl ToHttpResponse for HttpHandlerBindingError {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &GatewaySessionStore,
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
impl ToHttpResponse for CorsPreflight {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        let mut response = poem::Response::builder().status(StatusCode::OK).finish();

        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            self.get_allow_origin().clone().parse().unwrap(),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            self.get_allow_methods().clone().parse().unwrap(),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            self.get_allow_headers().clone().parse().unwrap(),
        );

        if let Some(expose_headers) = &self.get_expose_headers() {
            response.headers_mut().insert(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                expose_headers.clone().parse().unwrap(),
            );
        }

        if let Some(allow_credentials) = self.get_allow_credentials() {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                allow_credentials.to_string().parse().unwrap(),
            );
        }

        if let Some(max_age) = self.get_max_age() {
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
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        match internal::IntermediateHttpResponse::from(&self) {
            Ok(intermediate_response) => intermediate_response.to_http_response(request_details),
            Err(e) => e.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
}

#[async_trait]
impl ToHttpResponse for AuthorisationSuccess {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        let access_token = self.access_token;
        let id_token = self.id_token;
        let session_id = self.session;

        let mut response = poem::Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", self.target_path)
            .header(
                "Set-Cookie",
                format!(
                    "access_token={}; HttpOnly; Secure; Path=/; SameSite=None",
                    access_token
                )
                .as_str(),
            );

        if let Some(id_token) = id_token {
            response = response.header(
                "Set-Cookie",
                format!(
                    "id_token={}; HttpOnly; Secure; Path=/; SameSite=None",
                    id_token
                )
                .as_str(),
            )
        }

        response = response.header(
            "Set-Cookie",
            format!(
                "session_id={}; HttpOnly; Secure; Path=/; SameSite=None",
                session_id
            )
            .as_str(),
        );

        response.body(())
    }
}

#[async_trait]
impl ToHttpResponse for AuthorisationError {
    async fn to_response(
        self,
        _request_details: &RichRequest,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        self.to_response_from_safe_display(|_| StatusCode::UNAUTHORIZED)
    }
}

mod internal {
    use crate::gateway_execution::http_content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use crate::gateway_execution::request::RichRequest;
    use crate::gateway_rib_interpreter::RibRuntimeError;
    use http::StatusCode;

    use crate::getter::{get_response_headers_or_default, get_status_code_or_ok, GetterExt};
    use crate::path::Path;

    use crate::headers::ResolvedResponseHeaders;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use poem::{Body, IntoResponse, ResponseParts};
    use rib::RibResult;

    #[derive(Debug)]
    pub(crate) struct IntermediateHttpResponse {
        body: Option<TypeAnnotatedValue>,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateHttpResponse {
        pub(crate) fn from(
            evaluation_result: &RibResult,
        ) -> Result<IntermediateHttpResponse, RibRuntimeError> {
            match evaluation_result {
                RibResult::Val(rib_result) => {
                    let status = get_status_code_or_ok(rib_result).map_err(RibRuntimeError)?;

                    let headers =
                        get_response_headers_or_default(rib_result).map_err(RibRuntimeError)?;

                    let tav: TypeAnnotatedValue = rib_result
                        .clone()
                        .try_into()
                        .map_err(|errs: Vec<String>| RibRuntimeError(errs.join(", ")))?;

                    let body = tav
                        .get_optional(&Path::from_key("body"))
                        .unwrap_or(tav.clone());

                    Ok(IntermediateHttpResponse {
                        body: Some(body),
                        status,
                        headers,
                    })
                }
                RibResult::Unit => Ok(IntermediateHttpResponse {
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

    use crate::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySession, GatewaySessionError, SessionId,
    };
    use crate::gateway_execution::request::RichRequest;
    use crate::gateway_execution::to_response::ToHttpResponse;
    use golem_wasm_ast::analysis::analysed_type::record;
    use golem_wasm_ast::analysis::NameTypePair;
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
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

        let session_store: Arc<dyn GatewaySession + Send + Sync> = Arc::new(TestSessionStore);

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

        let session_store: Arc<dyn GatewaySession + Send + Sync> = Arc::new(TestSessionStore);

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
    impl GatewaySession for TestSessionStore {
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
