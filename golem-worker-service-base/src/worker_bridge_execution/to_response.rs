use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::{
    RefinedWorkerResponse, WorkerRequest, WorkerRequestExecutorError, WorkerResponse,
};
use http::StatusCode;
use poem::Body;

pub trait ToResponse<A> {
    fn to_response(
        &self,
        worker_request: &WorkerRequest,
        response_mapping: &Option<ResponseMapping>,
        request_details: &RequestDetails,
    ) -> A;
}

impl ToResponse<poem::Response> for WorkerResponse {
    fn to_response(
        &self,
        worker_request: &WorkerRequest,
        response_mapping: &Option<ResponseMapping>,
        request_details: &RequestDetails,
    ) -> poem::Response {
        let refined_worker_response = RefinedWorkerResponse::from_worker_response(self);

        match refined_worker_response {
            Ok(refined) => {
                match internal::IntermediateHttpResponse::from(
                    &refined,
                    response_mapping,
                    request_details,
                    worker_request,
                ) {
                    Ok(intermediate_response) => {
                        intermediate_response.to_http_response(request_details)
                    }
                    Err(e) => poem::Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from_string(format!(
                            "Error when  converting worker response to http response. Error: {}",
                            e
                        ))),
                }
            }
            Err(e) => poem::Response::builder()
                .status(poem::http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(poem::Body::from_string(
                    format!("API request error {}", e).to_string(),
                )),
        }
    }
}

impl ToResponse<poem::Response> for WorkerRequestExecutorError {
    fn to_response(
        &self,
        _worker_request: &WorkerRequest,
        _response_mapping: &Option<ResponseMapping>,
        _request_details: &RequestDetails,
    ) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(
                format!("API request error {}", self).to_string(),
            ))
    }
}

mod internal {
    use crate::api_definition::http::HttpResponseMapping;
    use crate::evaluator::Evaluator;
    use crate::evaluator::{EvaluationContext, EvaluationError, EvaluationResult};
    use crate::expression::Expr;
    use crate::primitive::{GetPrimitive, Primitive};
    use crate::worker_binding::{RequestDetails, ResponseMapping};
    use crate::worker_bridge_execution::content_type_mapper::GetHttpResponseBody;
    use crate::worker_bridge_execution::worker_bridge_response::RefinedWorkerResponse;
    use crate::worker_bridge_execution::WorkerRequest;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use http::{HeaderMap, StatusCode};
    
    use poem::{Body, ResponseParts};
    use std::collections::HashMap;

    pub(crate) struct IntermediateHttpResponse {
        body: EvaluationResult,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateHttpResponse {
        pub(crate) fn from(
            worker_response: &RefinedWorkerResponse,
            response_mapping: &Option<ResponseMapping>,
            request_details: &RequestDetails,
            worker_request: &WorkerRequest,
        ) -> Result<IntermediateHttpResponse, EvaluationError> {
            let evaluation_context =
                EvaluationContext::from(worker_request, worker_response, request_details);

            if let Some(res_map) = response_mapping {
                let http_response_mapping = HttpResponseMapping::try_from(res_map)
                    .map_err(EvaluationError::Message)?;

                let status_code =
                    get_status_code(&http_response_mapping.status, &evaluation_context)?;

                let headers = ResolvedResponseHeaders::from(
                    &http_response_mapping.headers,
                    &evaluation_context,
                )?;

                let evaluation_result = http_response_mapping.body.evaluate(&evaluation_context)?;

                Ok(IntermediateHttpResponse {
                    body: evaluation_result,
                    status: status_code,
                    headers,
                })
            } else {
                Ok(IntermediateHttpResponse {
                    body: EvaluationResult::from(worker_response),
                    status: StatusCode::default(),
                    headers: ResolvedResponseHeaders::default(),
                })
            }
        }
        pub(crate) fn to_http_response(&self, request_details: &RequestDetails) -> poem::Response {
            let headers: Result<HeaderMap, String> = (&self.headers.headers)
                .try_into()
                .map_err(|e: hyper::http::Error| e.to_string());

            let status = &self.status;
            let eval_result = &self.body;

            match headers {
                Ok(response_headers) => {
                    let parts = ResponseParts {
                        status: *status,
                        version: Default::default(),
                        headers: response_headers,
                        extensions: Default::default(),
                    };

                    match eval_result {
                        EvaluationResult::Value(type_annotated_value) => {
                            let content_type_opt = match request_details {
                                RequestDetails::Http(http_req) => http_req.get_content_type(),
                            };

                            match type_annotated_value.to_response_body(&content_type_opt) {
                                Ok(body) => poem::Response::from_parts(parts, body),
                                Err(content_map_error) => poem::Response::builder()
                                    .status(StatusCode::BAD_REQUEST)
                                    .body(Body::from_string(content_map_error.to_string())),
                            }
                        }
                        EvaluationResult::Unit => poem::Response::from_parts(parts, Body::empty()),
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

    fn get_status_code(
        status_expr: &Expr,
        evaluator_context: &EvaluationContext,
    ) -> Result<StatusCode, EvaluationError> {
        let status_value = status_expr.evaluate(evaluator_context)?;
        let status_res: Result<u16, EvaluationError> =
            match status_value.get_primitive() {
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
                    status_value
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
        // Example: In API definition, user may define a header as "X-Request-${worker-response.value}" to be added
        // to the http response. Here we resolve the expression based on the resolved variables (that was formed from the response of the worker)
        fn from(
            header_mapping: &HashMap<String, Expr>,
            input: &EvaluationContext, // The input to evaluating header expression is a type annotated value
        ) -> Result<ResolvedResponseHeaders, EvaluationError> {
            let mut resolved_headers: HashMap<String, String> = HashMap::new();

            for (header_name, header_value_expr) in header_mapping {
                let value = header_value_expr
                    .evaluate(input)?
                    .get_value()
                    .ok_or("Unable to resolve header. Resulted in ()".to_string())?;

                let value_str = value
                    .get_primitive()
                    .ok_or(EvaluationError::Message(format!(
                        "Header value is not a string. {}",
                        get_json_from_typed_value(&value)
                    )))?;

                resolved_headers.insert(header_name.clone(), value_str.to_string());
            }

            Ok(ResolvedResponseHeaders {
                headers: resolved_headers,
            })
        }
    }
}
