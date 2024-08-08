use crate::evaluator::{EvaluationError, ExprEvaluationResult};
use crate::worker_binding::RequestDetails;

use http::StatusCode;
use poem::Body;

pub trait ToResponse<A> {
    fn to_response(&self, request_details: &RequestDetails) -> A;
}

impl ToResponse<poem::Response> for ExprEvaluationResult {
    fn to_response(&self, request_details: &RequestDetails) -> poem::Response {
        match internal::IntermediateHttpResponse::from(self) {
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

impl ToResponse<poem::Response> for EvaluationError {
    fn to_response(&self, _request_details: &RequestDetails) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(format!("Error {}", self).to_string()))
    }
}

impl ToResponse<poem::Response> for String {
    fn to_response(&self, _request_details: &RequestDetails) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(self.to_string()))
    }
}

mod internal {
    use crate::evaluator::{EvaluationError, ExprEvaluationResult};
    use crate::primitive::{GetPrimitive, Primitive};
    use crate::worker_binding::RequestDetails;
    use crate::worker_bridge_execution::content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use http::{HeaderMap, StatusCode};
    use std::str::FromStr;

    use crate::evaluator::getter::GetterExt;
    use crate::evaluator::path::Path;
    use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::TypedRecord;
    use poem::web::headers::ContentType;
    use poem::{Body, IntoResponse, ResponseParts};
    use std::collections::HashMap;

    pub(crate) struct IntermediateHttpResponse {
        body: Option<TypeAnnotatedValue>,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateHttpResponse {
        pub(crate) fn from(
            evaluation_result: &ExprEvaluationResult,
        ) -> Result<IntermediateHttpResponse, EvaluationError> {
            match evaluation_result {
                ExprEvaluationResult::Value(typed_value) => {
                    let status = match typed_value.get_optional(&Path::from_key("status")) {
                        Some(typed_value) => get_status_code(&typed_value),
                        None => Ok(StatusCode::OK),
                    }?;

                    let headers = match typed_value.get_optional(&Path::from_key("headers")) {
                        None => Ok(ResolvedResponseHeaders::default()),
                        Some(header) => ResolvedResponseHeaders::from_typed_value(&header),
                    }?;

                    let body = typed_value
                        .get_optional(&Path::from_key("body"))
                        .unwrap_or(typed_value.clone());

                    Ok(IntermediateHttpResponse {
                        body: Some(body),
                        status,
                        headers,
                    })
                }
                ExprEvaluationResult::Unit => Ok(IntermediateHttpResponse {
                    body: None,
                    status: StatusCode::default(),
                    headers: ResolvedResponseHeaders::default(),
                }),
            }
        }

        pub(crate) fn to_http_response(&self, request_details: &RequestDetails) -> poem::Response {
            let headers: Result<HeaderMap, String> = (&self.headers.headers)
                .try_into()
                .map_err(|e: hyper::http::Error| e.to_string());

            let status = &self.status;
            let evaluation_result = &self.body;

            match headers {
                Ok(response_headers) => {
                    let response_content_type =
                        get_content_type_from_response_headers(&response_headers);

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
                Err(err) => poem::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(format!(
                        "Unable to resolve valid headers. Error: {}",
                        err
                    ))),
            }
        }
    }

    fn get_content_type_from_response_headers(response_headers: &HeaderMap) -> Option<ContentType> {
        response_headers
            .get(http::header::CONTENT_TYPE.to_string())
            .and_then(|header_value| {
                header_value
                    .to_str()
                    .ok()
                    .and_then(|header_str| ContentType::from_str(header_str).ok())
            })
    }

    fn get_status_code(status_code: &TypeAnnotatedValue) -> Result<StatusCode, EvaluationError> {
        let status_res: Result<u16, EvaluationError> =
            match status_code.get_primitive() {
                Some(Primitive::String(status_str)) => status_str.parse().map_err(|e| {
                    EvaluationError::Message(format!(
                        "Invalid Status Code Expression. It is resolved to a string but not a number {}. Error: {}",
                        status_str, e
                    ))
                }),
                Some(Primitive::Num(number)) => number.to_string().parse().map_err(|e| {
                    EvaluationError::Message(format!(
                        "Invalid Status Code Expression. It is resolved to a number but not a u16 {}. Error: {}",
                        number, e
                    ))
                }),
                _ => Err(EvaluationError::Message(format!(
                    "Status Code Expression is evaluated to a complex value. It is resolved to {:?}",
                    status_code.to_json_value()
                )))
            };

        let status_u16 = status_res?;

        StatusCode::from_u16(status_u16).map_err(|e| EvaluationError::Message(format!(
            "Invalid Status Code. A valid status code cannot be formed from the evaluated status code expression {}. Error: {}",
            status_u16, e
        )))
    }

    #[derive(Default, Debug, PartialEq)]
    pub(crate) struct ResolvedResponseHeaders {
        pub(crate) headers: HashMap<String, String>,
    }

    impl ResolvedResponseHeaders {
        pub fn from_typed_value(
            header_map: &TypeAnnotatedValue,
        ) -> Result<ResolvedResponseHeaders, String> {
            match header_map {
                TypeAnnotatedValue::Record(TypedRecord { value, .. }) => {
                    let mut resolved_headers: HashMap<String, String> = HashMap::new();

                    for name_value_pair in value {
                        let value_str = name_value_pair
                            .value
                            .as_ref()
                            .and_then(|v| v.type_annotated_value.clone())
                            .ok_or("Unable to resolve header value".to_string())?
                            .get_primitive()
                            .map(|primitive| primitive.to_string())
                            .unwrap_or_else(|| "Unable to resolve header".to_string());

                        resolved_headers.insert(name_value_pair.name.clone(), value_str);
                    }

                    Ok(ResolvedResponseHeaders {
                        headers: resolved_headers,
                    })
                }

                _ => Err(format!(
                    "Header expression is not a record. It is resolved to {}",
                    header_map.to_json_value()
                )),
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::worker_binding::TypedHttRequestDetails;
    use crate::worker_bridge_execution::to_response::internal::ResolvedResponseHeaders;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::Type;
    use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, TypedRecord};
    use http::header::CONTENT_TYPE;
    use std::collections::HashMap;

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

    #[tokio::test]
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

        let evaluation_result: ExprEvaluationResult = ExprEvaluationResult::Value(record);

        let http_response: poem::Response =
            evaluation_result.to_response(&RequestDetails::Http(TypedHttRequestDetails::empty()));

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

    #[tokio::test]
    async fn test_evaluation_result_to_response_with_no_http_specifics() {
        let evaluation_result: ExprEvaluationResult =
            ExprEvaluationResult::Value(TypeAnnotatedValue::Str("Healthy".to_string()));

        let http_response: poem::Response =
            evaluation_result.to_response(&RequestDetails::Http(TypedHttRequestDetails::empty()));

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

    #[test]
    fn test_get_response_headers_from_typed_value() {
        let header_map = create_record(vec![
            (
                "header1".to_string(),
                TypeAnnotatedValue::Str("value1".to_string()),
            ),
            ("header2".to_string(), TypeAnnotatedValue::F32(1.0)),
        ]);

        let resolved_headers = ResolvedResponseHeaders::from_typed_value(&header_map).unwrap();

        let mut map = HashMap::new();

        map.insert("header1".to_string(), "value1".to_string());
        map.insert("header2".to_string(), "1".to_string());

        let expected = ResolvedResponseHeaders { headers: map };

        assert_eq!(resolved_headers, expected)
    }
}
