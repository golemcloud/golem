use crate::api::WorkerApiBaseError;
use crate::gateway_binding::{GatewayRequestDetails, RibInputTypeMismatch};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingError, FileServerBindingResult,
};
use crate::gateway_execution::gateway_session::{
    DataKey, DataValue, GatewaySessionStore, SessionId,
};
use crate::gateway_identity_provider::{IdentityProvider, IdentityProviderError};
use crate::gateway_middleware::{Cors as CorsPreflight, Middlewares, OpenIdProviderDetailsWithClient};
use crate::gateway_rib_interpreter::EvaluationError;
use async_trait::async_trait;
use http::header::*;
use http::StatusCode;
use openidconnect::{AuthorizationCode, Nonce, OAuth2TokenResponse};
use poem::Body;
use poem::IntoResponse;
use rib::RibResult;

#[async_trait]
pub trait ToResponse<A> {
    async fn to_response(
        self,
        request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        session_store: &GatewaySessionStore,
    ) -> A;
}

#[async_trait]
impl ToResponse<poem::Response> for FileServerBindingResult {
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: &GatewaySessionStore,
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
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        _middlewares: &Middlewares,
        _session_store: GatewaySessionStore,
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
    async fn to_response(
        self,
        request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: &GatewaySessionStore,
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
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore,
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
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: GatewaySessionStore,
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
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        let mut response = poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(self.to_string()));

        middlewares.transform_http_response(&mut response);
        response
    }
}

impl ToResponse<poem::Response> for IdentityProviderError {
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        _middlewares: &Middlewares,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        match self {
            IdentityProviderError::FailedToDiscoverProviderMetadata(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", e).to_string())),
            IdentityProviderError::FailedToExchangeCodeForTokens(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", e).to_string())),
            IdentityProviderError::IdTokenVerificationError(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", e).to_string())),
            IdentityProviderError::ClientInitError(error) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", error).to_string())),
            IdentityProviderError::InvalidIssuerUrl(error) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", error).to_string())),
        }
    }
}

impl<T, E> ToResponse<poem::Response> for Result<T, E> {
    fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        _middlewares: &Middlewares,
        _session_store: GatewaySessionStore,
    ) -> poem::Response {
        match self {
            Ok(_) => poem::Response::builder()
                .status(StatusCode::OK)
                .body(Body::empty()),
            Err(_) => poem::Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from_string("Unauthorised auth call back".to_string())),
        }
    }
}

struct AuthorisationError(String);
impl ToResponse<poem::Response> for AuthorisationError {
    async fn to_response(
        self,
        _request_details: &GatewayRequestDetails,
        _middlewares: &Middlewares,
        _session_store: &GatewaySessionStore,
    ) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from_string(self.0))
    }
}

#[async_trait]
impl ToResponse<poem::Response> for OpenIdProviderDetailsWithClient {
    async fn to_response(
        self,
        request_details: &GatewayRequestDetails,
        middlewares: &Middlewares,
        session_store: GatewaySessionStore,
    ) -> poem::Response {
        match request_details {
            GatewayRequestDetails::Http(http_request_details) => {
                let response_result =
                    internal::handle_auth(self, request_details, http_request_details, middlewares, session_store);

                response_result.await.unwrap_or_else(|response| response)
            }
        }
    }
}

mod internal {
    use std::future::Future;
    use std::pin::Pin;
    use crate::gateway_binding::{GatewayRequestDetails, HttpRequestDetails};
    use crate::gateway_execution::http_content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use crate::gateway_rib_interpreter::EvaluationError;
    use http::StatusCode;

    use crate::getter::{get_response_headers_or_default, get_status_code_or_ok, GetterExt};
    use crate::path::Path;

    use crate::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySessionStore, SessionId,
    };
    use crate::gateway_execution::to_response::{AuthorisationError, ToResponse};
    use crate::gateway_middleware::{Middlewares, OpenIdProviderDetailsWithClient};
    use crate::headers::ResolvedResponseHeaders;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use openidconnect::{AuthorizationCode, Nonce, OAuth2TokenResponse};
    use poem::{Body, IntoResponse, ResponseParts};
    use rib::RibResult;


    trait AsyncErrExtension<A, B, C, F, G> {
        async fn execute(self, f: F, g: G) -> Result<C, C>
        where
            F: FnOnce(A) -> Pin<Box<dyn Future<Output = C>>>,
            G: FnOnce(B) -> Pin<Box<dyn Future<Output = C>>>,
            A: 'static,
            B: 'static,
            C: 'static,
            Self: Sized; // Self is the Future that returns a Result<A, B>
    }

    impl<F, A, B, C, G> AsyncErrExtension<F, A, B, C, G> for F
    where
        F: Future<Output = Result<A, B>>,
        A: 'static,
        B: 'static,
        C: 'static,
        G: FnOnce(B) -> Pin<Box<dyn Future<Output = C>>>,
        F: Sized,
    {
        async fn execute(self, f: G, g: G) -> Result<A, C> {
            match self.await {
                Ok(val) =>  Ok(val),
                Err(err) => Err(g(err).await),
            }
        }
    }

    // TODO; Move out of here
    pub(crate) async fn handle_auth(
        auth_client: OpenIdProviderDetailsWithClient,
        request_details: &GatewayRequestDetails,
        http_request_details: &HttpRequestDetails,
        middlewares: &Middlewares,
        session_store: GatewaySessionStore,
    ) -> Result<poem::Response, poem::Response> {
        let query_params = &http_request_details.request_path_values;
        let code = query_params
            .get("code")
            .ok_or(AuthorisationError(
                "Unauthorised auth call back".to_string(),
            ))
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let code = code
            .as_str()
            .ok_or(AuthorisationError(
                "Unauthorised auth call back".to_string(),
            ))
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let authorisation_code = AuthorizationCode::new(code.to_string());
        let state_value = query_params
            .get("state")
            .ok_or(AuthorisationError(
                "Unauthorised auth call back".to_string(),
            ))
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let state_str = state_value
            .as_str()
            .ok_or(AuthorisationError(
                "Unauthorised auth call back".to_string(),
            ))
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let obtained_state = state_str.to_string();
        let session_params = session_store
            .0
            .get_params(SessionId(obtained_state.to_string()))
            .await
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?
            .ok_or(AuthorisationError(
                "Unauthorised auth call back".to_string(),
            ))
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let nonce = session_params
            .get(&DataKey("nonce".to_string()))
            .ok_or(AuthorisationError(
                "Unauthorised auth call back".to_string(),
            ))
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?
            .0
            .clone();

        let open_id_client = auth_client
            .identity_provider
            .get_client(
                &auth_client.provider_metadata.clone(),
                &auth_client.security_scheme_name.clone(),
            )
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let token_response = auth_client
            .identity_provider
            .exchange_code_for_tokens(&open_id_client, &authorisation_code)
            .await
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let claims = auth_client
            .identity_provider
            .get_claims(
                &open_id_client,
                token_response.clone(),
                &Nonce::new(nonce.clone()),
            )
            .map_err(|err| err.to_response(request_details, middlewares, &session_store))?;

        let _ = session_store
            .0
            .insert(
                SessionId(obtained_state.to_string()),
                DataKey("claims".to_string()),
                DataValue(claims.to_string()), // TODO;
            )
            .await;

        let access_token = token_response.access_token().secret().clone();

        // access token in session store
        let _ = session_store
            .0
            .insert(
                SessionId(obtained_state.to_string()),
                DataKey("access_token".to_string()),
                DataValue(access_token),
            )
            .await;

        let mut response = poem::Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", "/")
            .header(
                "Authorization",
                format!("Bearer {}", &token_response.access_token().secret().clone()),
            )
            .body(());

        middlewares.transform_http_response(&mut response);

        Ok(response)
    }

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
    use crate::gateway_execution::gateway_session::GatewaySessionStore;

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
            &GatewaySessionStore::in_memory(),
        ).await;

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
