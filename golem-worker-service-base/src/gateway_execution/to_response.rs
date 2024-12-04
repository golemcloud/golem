// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::api::WorkerApiBaseError;
use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::auth_call_back_binding_handler::AuthCallBackResult;
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingError, FileServerBindingResult,
};
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_middleware::HttpCors as CorsPreflight;
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
        request_details: &HttpRequestDetails,
        session_store: &GatewaySessionStore,
    ) -> poem::Response;
}

#[async_trait]
impl ToHttpResponse for FileServerBindingResult {
    async fn to_response(
        self,
        _request_details: &HttpRequestDetails,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        match self {
            Ok(data) => Body::from_bytes_stream(data.data)
                .with_content_type(data.binding_details.content_type.to_string())
                .with_status(data.binding_details.status_code)
                .into_response(),
            Err(FileServerBindingError::InternalError(e)) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", e).to_string())),
            Err(FileServerBindingError::ComponentServiceError(inner)) => {
                WorkerApiBaseError::from(inner).into_response()
            }
            Err(FileServerBindingError::WorkerServiceError(inner)) => {
                WorkerApiBaseError::from(inner).into_response()
            }
            Err(FileServerBindingError::InvalidRibResult(e)) => poem::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string(
                    format!("Error while processing rib result: {}", e).to_string(),
                )),
        }
    }
}

// Preflight (OPTIONS) response that will consist of all configured CORS headers
#[async_trait]
impl ToHttpResponse for CorsPreflight {
    async fn to_response(
        self,
        _request_details: &HttpRequestDetails,
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
        request_details: &HttpRequestDetails,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        match internal::IntermediateHttpResponse::from(&self) {
            Ok(intermediate_response) => intermediate_response.to_http_response(request_details),
            Err(e) => e.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
}

#[async_trait]
impl ToHttpResponse for AuthCallBackResult {
    async fn to_response(
        self,
        _request_details: &HttpRequestDetails,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        match self {
            Ok(success) => {
                let access_token = success.access_token;
                let id_token = success.id_token;
                let session_id = success.session;

                let mut response = poem::Response::builder()
                    .status(StatusCode::FOUND)
                    .header("Location", success.target_path)
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

            Err(err) => err.to_response_from_safe_display(|_| StatusCode::UNAUTHORIZED),
        }
    }
}

mod internal {
    use crate::gateway_binding::HttpRequestDetails;
    use crate::gateway_execution::http_content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use crate::gateway_rib_interpreter::EvaluationError;
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
        ) -> Result<IntermediateHttpResponse, EvaluationError> {
            match evaluation_result {
                RibResult::Val(rib_result) => {
                    let status = get_status_code_or_ok(rib_result).map_err(EvaluationError)?;

                    let headers =
                        get_response_headers_or_default(rib_result).map_err(EvaluationError)?;

                    let body = rib_result
                        .get_optional(&Path::from_key("body"))
                        .unwrap_or(rib_result.clone());

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

        pub(crate) fn to_http_response(
            &self,
            request_details: &HttpRequestDetails,
        ) -> poem::Response {
            let response_content_type = self.headers.get_content_type();
            let response_headers = self.headers.headers.clone();

            let status = &self.status;
            let evaluation_result = &self.body;

            let accepted_content_types = request_details.get_accept_content_type_header();

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
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::Type;
    use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, TypedRecord};
    use std::sync::Arc;
    use test_r::test;

    use crate::gateway_binding::HttpRequestDetails;
    use crate::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySession, GatewaySessionError, SessionId,
    };
    use crate::gateway_execution::to_response::ToHttpResponse;
    use http::header::CONTENT_TYPE;
    use http::StatusCode;
    use rib::RibResult;

    fn create_record(values: Vec<(String, TypeAnnotatedValue)>) -> TypeAnnotatedValue {
        let mut name_type_pairs = vec![];
        let mut name_value_pairs = vec![];

        for (key, value) in values.iter() {
            let typ = Type::try_from(value).unwrap();
            name_type_pairs.push(NameTypePair {
                name: key.to_string(),
                typ: Some(typ),
            });

            name_value_pairs.push(NameValuePair {
                name: key.to_string(),
                value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(value.clone()),
                }),
            });
        }

        TypeAnnotatedValue::Record(TypedRecord {
            typ: name_type_pairs,
            value: name_value_pairs,
        })
    }

    #[test]
    async fn test_evaluation_result_to_response_with_http_specifics() {
        let record = create_record(vec![
            ("status".to_string(), TypeAnnotatedValue::U16(400)),
            (
                "headers".to_string(),
                create_record(vec![(
                    "Content-Type".to_string(),
                    TypeAnnotatedValue::Str("application/json".to_string()),
                )]),
            ),
            (
                "body".to_string(),
                TypeAnnotatedValue::Str("Hello".to_string()),
            ),
        ]);

        let evaluation_result: RibResult = RibResult::Val(record);

        let session_store: Arc<dyn GatewaySession + Send + Sync> = Arc::new(TestSessionStore);

        let http_response: poem::Response = evaluation_result
            .to_response(&HttpRequestDetails::empty(), &session_store)
            .await;

        let (response_parts, body) = http_response.into_parts();
        let body = body.into_string().await.unwrap();
        let headers = response_parts.headers;
        let status = response_parts.status;

        let expected_body = "Hello";
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
        let evaluation_result: RibResult =
            RibResult::Val(TypeAnnotatedValue::Str("Healthy".to_string()));

        let session_store: Arc<dyn GatewaySession + Send + Sync> = Arc::new(TestSessionStore);

        let http_response: poem::Response = evaluation_result
            .to_response(&HttpRequestDetails::empty(), &session_store)
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
