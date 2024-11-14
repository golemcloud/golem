use std::collections::HashMap;
use async_trait::async_trait;
use crate::api::WorkerApiBaseError;
use crate::gateway_binding::{AuthCallBack, GatewayRequestDetails, RibInputTypeMismatch};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingError, FileServerBindingResult,
};
use crate::gateway_middleware::{Cors as CorsPreflight, Middlewares};
use crate::gateway_rib_interpreter::EvaluationError;
use http::header::*;
use http::StatusCode;
use openidconnect::AuthorizationCode;
use poem::{Body, Response};
use poem::IntoResponse;
use rib::RibResult;
use crate::gateway_execution::gateway_session::{DataKey, DataValue, GatewaySessionStore, SessionId};

#[async_trait]
pub trait ToResponse<A> {
    async fn to_response(self, request_details: &GatewayRequestDetails, middlewares: &Middlewares, session_store: GatewaySessionStore) -> A;
}

#[async_trait]
impl ToResponse<poem::Response> for FileServerBindingResult {
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore
    ) -> poem::Response {
        let mut response = match self {
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
        };

        middlewares.transform_http_response(&mut response);
        response
    }
}

// Preflight (OPTIONS) response that will consist of all configured CORS headers
#[async_trait]
impl ToResponse<poem::Response> for CorsPreflight {
    fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        _middlewares: &Middlewares,
        _session_store: GatewaySessionStore
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
impl ToResponse<poem::Response> for RibResult {
    fn to_response(
        self,
        request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore
    ) -> poem::Response {
        match internal::IntermediateHttpResponse::from(&self) {
            Ok(intermediate_response) => {
                intermediate_response.to_http_response(request_details, middlewares)
            }
            Err(e) => poem::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string(format!(
                    "Error when  converting worker response to http response. Error: {}",
                    e
                ))),
        }
    }
}

#[async_trait]
impl ToResponse<poem::Response> for RibInputTypeMismatch {
    fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore
    ) -> poem::Response {
        let mut response = poem::Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from_string(format!("Error {}", self.0).to_string()));

        middlewares.transform_http_response(&mut response);
        response
    }
}

#[async_trait]
impl ToResponse<poem::Response> for EvaluationError {
    fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore
    ) -> poem::Response {
        let mut response = poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(format!("Error {}", self).to_string()));

        middlewares.transform_http_response(&mut response);

        response
    }
}

#[async_trait]
impl ToResponse<poem::Response> for String {
    fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore
    ) -> poem::Response {
        let mut response = poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(self.to_string()));

        middlewares.transform_http_response(&mut response);
        response
    }
}

#[async_trait]
impl ToResponse<poem::Response> for AuthCallBack {
    async fn to_response(
        self,
        request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        session_store: GatewaySessionStore
    ) -> poem::Response {
        match request_details {
            GatewayRequestDetails::Http(http_request_details) => {
                let query_params = &http_request_details.request_path_values;
                let code = query_params.get("code");

                match code {
                    None => Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(Body::from_string("Unauthorised auth call back".to_string())),
                    Some(code) => {
                        let code = code.as_str();
                        match code {
                            None => Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .body(Body::from_string("Unauthorised auth call back".to_string())),
                            Some(code) => {
                                let authorisation_code =
                                    AuthorizationCode::new(code.to_string());
                                let state = query_params.get("state");

                                if let Some(state) = state {
                                    let state = state.as_str();
                                    if let Some(state) = state {
                                        let obtained_state = state.to_string();
                                        let existing_state =
                                            session_store.0.get_params(SessionId("state".to_string())).await;

                                        match existing_state {
                                            Ok(Some(session_params)) => {
                                                let original_uri = session_params.get(&DataKey("original_uri".to_string())).unwrap();
                                                let client =

                                            }
                                            Err(_) => {
                                                return Response::builder()
                                                    .status(StatusCode::UNAUTHORIZED)
                                                    .body(Body::from_string("Unauthorised auth call back".to_string()))
                                            }
                                        }



                                    }
                                }

                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Body::from_string("Authorised auth call back".to_string()))
                            }
                        }
                        let authorisation_code = AuthorizationCode::new(code.as_str().);
                    }
                }
            }
        }
    }
}

mod internal {
    use crate::gateway_binding::GatewayRequestDetails;
    use crate::gateway_execution::http_content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use crate::gateway_rib_interpreter::EvaluationError;
    use http::StatusCode;

    use crate::getter::{get_response_headers_or_default, get_status_code_or_ok, GetterExt};
    use crate::path::Path;

    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    use crate::gateway_middleware::Middlewares;
    use crate::headers::ResolvedResponseHeaders;
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
            request_details: &GatewayRequestDetails,
            middleware: &Middlewares,
        ) -> poem::Response {
            let response_content_type = self.headers.get_content_type();
            let response_headers = self.headers.headers.clone();

            let status = &self.status;
            let evaluation_result = &self.body;

            let accepted_content_types = match request_details {
                GatewayRequestDetails::Http(http) => http.get_accept_content_type_header(),
            };

            let content_type =
                ContentTypeHeaders::from(response_content_type, accepted_content_types);

            let mut response = match evaluation_result {
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

            middleware.transform_http_response(&mut response);
            response
        }
    }
}

#[cfg(test)]
mod test {
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::Type;
    use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, TypedRecord};
    use test_r::test;

    use crate::gateway_binding::{GatewayRequestDetails, HttpRequestDetails};
    use crate::gateway_execution::to_response::ToResponse;
    use crate::gateway_middleware::Middlewares;
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

        let http_response: poem::Response = evaluation_result.to_response(
            &GatewayRequestDetails::Http(HttpRequestDetails::empty()),
            &Middlewares::default(),
        );

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

        let http_response: poem::Response = evaluation_result.to_response(
            &GatewayRequestDetails::Http(HttpRequestDetails::empty()),
            &Middlewares::default(),
        );

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
}
