use crate::api::WorkerApiBaseError;
use crate::worker_binding::fileserver_binding_handler::{FileServerBindingError, FileServerBindingResult};
use crate::worker_binding::{RequestDetails, RibInputTypeMismatch};
use crate::worker_service_rib_interpreter::EvaluationError;
use http::StatusCode;
use poem::Body;
use rib::RibResult;
use poem::IntoResponse;

pub trait ToResponse<A> {
    fn to_response(self, request_details: &RequestDetails) -> A;
}

impl ToResponse<poem::Response> for RibResult {
    fn to_response(self, request_details: &RequestDetails) -> poem::Response {
        match internal::IntermediateHttpResponse::from(&self) {
            Ok(intermediate_response) => intermediate_response.to_http_response(request_details),
            Err(e) => poem::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string(format!(
                    "Error when  converting worker response to http response. Error: {}",
                    e
                ))),
        }
    }
}

impl ToResponse<poem::Response> for RibInputTypeMismatch {
    fn to_response(self, _request_details: &RequestDetails) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from_string(format!("Error {}", self.0).to_string()))
    }
}

impl ToResponse<poem::Response> for EvaluationError {
    fn to_response(self, _request_details: &RequestDetails) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(format!("Error {}", self).to_string()))
    }
}

impl ToResponse<poem::Response> for String {
    fn to_response(self, _request_details: &RequestDetails) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(self.to_string()))
    }
}

impl ToResponse<poem::Response> for FileServerBindingResult {
    fn to_response(self, _request_details: &RequestDetails) -> poem::Response {
        match self {
            Ok(data) =>
                Body::from_bytes_stream(data.data)
                    .with_content_type(&data.binding_details.content_type.to_string())
                    .with_status(data.binding_details.status_code)
                    .into_response(),
            Err(FileServerBindingError::InternalError(e)) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!("Error {}", e).to_string())),
            Err(FileServerBindingError::ComponentServiceError(inner)) =>
                WorkerApiBaseError::from(inner).into_response(),
            Err(FileServerBindingError::WorkerServiceError(inner)) =>
                WorkerApiBaseError::from(inner).into_response(),
        }
    }
}

mod internal {
    use crate::worker_binding::RequestDetails;
    use crate::worker_bridge_execution::content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use crate::worker_service_rib_interpreter::EvaluationError;
    use http::StatusCode;
    

    use crate::getter::{get_response_headers_or_default, get_status_code_or_ok, GetterExt};
    use crate::path::Path;
    
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    
    
    use poem::{Body, IntoResponse, ResponseParts};
    use rib::RibResult;
    
    use crate::headers::ResolvedResponseHeaders;
    

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
                RibResult::Val(typed_value) => {
                    let status = get_status_code_or_ok(typed_value)
                        .map_err(|e| EvaluationError(e))?;

                    let headers = get_response_headers_or_default(typed_value)
                        .map_err(|e| EvaluationError(e))?;

                    let body = typed_value
                        .get_optional(&Path::from_key("body"))
                        .unwrap_or(typed_value.clone());

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

        pub(crate) fn to_http_response(&self, request_details: &RequestDetails) -> poem::Response {
            let response_content_type = self.headers.get_content_type();
            let response_headers = self.headers.headers.clone();

            let status = &self.status;
            let evaluation_result = &self.body;

            let accepted_content_types = match request_details {
                RequestDetails::Http(http) => http.get_accept_content_type_header(),
            };

            let content_type =
                ContentTypeHeaders::from(response_content_type, accepted_content_types);

            match evaluation_result {
                Some(type_annotated_value) => {
                    match type_annotated_value.to_http_resp_with_content_type(content_type)
                    {
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
            }
        }
    }
}

#[cfg(test)]
mod test {
    use test_r::test;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::Type;
    use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, TypedRecord};

    use crate::worker_binding::{HttpRequestDetails, RequestDetails};
    use crate::worker_bridge_execution::to_response::ToResponse;
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

        let http_response: poem::Response =
            evaluation_result.to_response(&RequestDetails::Http(HttpRequestDetails::empty()));

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

        let http_response: poem::Response =
            evaluation_result.to_response(&RequestDetails::Http(HttpRequestDetails::empty()));

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
