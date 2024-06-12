use crate::evaluator::{EvaluationError, EvaluationResult, MetadataFetchError};
use crate::worker_binding::RequestDetails;

use http::StatusCode;
use poem::Body;

pub trait ToResponse<A> {
    fn to_response(&self, request_details: &RequestDetails) -> A;
}

impl ToResponse<poem::Response> for EvaluationResult {
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

impl ToResponse<poem::Response> for MetadataFetchError {
    fn to_response(&self, _request_details: &RequestDetails) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(
                format!("Worker metadata fetch error {}", self).to_string(),
            ))
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
    use crate::evaluator::{EvaluationError, EvaluationResult};
    use crate::primitive::{GetPrimitive, Primitive};
    use crate::worker_binding::RequestDetails;
    use crate::worker_bridge_execution::content_type_mapper::{
        ContentTypeHeaders, HttpContentTypeResponseMapper,
    };
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use http::{HeaderMap, StatusCode};
    use std::str::FromStr;

    use golem_wasm_rpc::TypeAnnotatedValue;
    use poem::{Body, IntoResponse, ResponseParts};
    use std::collections::HashMap;

    use crate::evaluator::getter::Getter;
    use crate::evaluator::path::Path;
    use poem::web::headers::ContentType;

    pub(crate) struct IntermediateHttpResponse {
        body: Option<TypeAnnotatedValue>,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateHttpResponse {
        pub(crate) fn from(
            worker_result: &EvaluationResult,
        ) -> Result<IntermediateHttpResponse, EvaluationError> {
            match worker_result {
                EvaluationResult::Value(type_annotated_value) => {
                    let status_typed = type_annotated_value.get(&Path::from_key("status"))?;
                    let status = get_status_code(&status_typed)?;
                    let body = type_annotated_value.get(&Path::from_key("body"))?;
                    let headers = ResolvedResponseHeaders::from_map(
                        &type_annotated_value.get(&Path::from_key("headers"))?,
                    )?;

                    Ok(IntermediateHttpResponse {
                        body: Some(body),
                        status,
                        headers,
                    })
                }
                EvaluationResult::Unit => Ok(IntermediateHttpResponse {
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
                            match type_annotated_value.to_http_response_body(content_type) {
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
                    get_json_from_typed_value(status_code)
                )))
            };

        let status_u16 = status_res?;

        StatusCode::from_u16(status_u16).map_err(|e| EvaluationError::Message(format!(
            "Invalid Status Code. A valid status code cannot be formed from the evaluated status code expression {}. Error: {}",
            status_u16, e
        )))
    }

    #[derive(Default)]
    struct ResolvedResponseHeaders {
        headers: HashMap<String, String>,
    }

    impl ResolvedResponseHeaders {
        pub fn from_map(
            header_map: &TypeAnnotatedValue,
        ) -> Result<ResolvedResponseHeaders, String> {
            match header_map {
                TypeAnnotatedValue::Record { value, .. } => {
                    let mut resolved_headers: HashMap<String, String> = HashMap::new();

                    for (header_name, header_value) in value {
                        let value_str = header_value
                            .get_primitive()
                            .map(|primitive| primitive.to_string())
                            .unwrap_or_else(|| {
                                format!(
                                    "Unable to resolve header. Resulted in {}",
                                    get_json_from_typed_value(header_value)
                                )
                            });

                        resolved_headers.insert(header_name.clone(), value_str);
                    }

                    Ok(ResolvedResponseHeaders {
                        headers: resolved_headers,
                    })
                }

                _ => Err(format!(
                    "Header expression is not a record. It is resolved to {}",
                    get_json_from_typed_value(header_map)
                )),
            }
        }
    }
}
